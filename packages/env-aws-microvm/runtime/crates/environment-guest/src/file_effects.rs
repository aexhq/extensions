//! Durable exact-pair fences for effectful live-file and storage-copy operations.
//!
//! Brain supplies a stable operation id and canonical request digest. The intent is synced before
//! any workspace mutation or external PUT. An exact retry replays only a committed result; an
//! intent without a result is ambiguous and is never executed again. The log is generation-local
//! and disappears only with the physical target.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

use environment_wire::{FileEffectIdentity, FileEffectKind, FileEffectStoredResult};
use serde::{Deserialize, Serialize};

pub const MAX_RETAINED_FILE_EFFECTS: usize = 1_024;
pub const MAX_FILE_EFFECT_LOG_BYTES: u64 = 64 * 1024 * 1024;
const RESULT_RESERVATION_BYTES: u64 = 32 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum EffectEvent {
    Intent {
        identity: FileEffectIdentity,
    },
    Complete {
        identity: FileEffectIdentity,
        result: Box<FileEffectStoredResult>,
    },
}

struct EffectRecord {
    identity: FileEffectIdentity,
    result: Option<FileEffectStoredResult>,
    /// Volatile authority to begin the effect. It is intentionally absent after process restart:
    /// a durable intent without a surviving pre-effect claimant is ambiguous and cannot run.
    claim_available: bool,
}

struct EffectWriter {
    file: File,
    bytes: u64,
    reserved_result_bytes: u64,
    dirty: bool,
    max_bytes: u64,
}

pub struct FileEffectStore {
    records: Mutex<HashMap<String, EffectRecord>>,
    writer: Mutex<EffectWriter>,
    max_effects: usize,
}

#[derive(Debug, Clone)]
pub enum EffectReservation {
    New,
    Replay(Box<FileEffectStoredResult>),
}

#[derive(Debug, thiserror::Error)]
pub enum FileEffectStoreError {
    #[error("file effect identity conflicts with the retained operation")]
    Conflict,
    #[error("file effect delivery is ambiguous and will not be repeated")]
    Ambiguous,
    #[error("file effect retention capacity is exhausted")]
    Capacity,
    #[error("file effect log is corrupt: {0}")]
    Corrupt(&'static str),
    #[error("invalid file effect identity: {0}")]
    Invalid(&'static str),
    #[error("file effect storage is unavailable")]
    Io(#[source] std::io::Error),
}

impl FileEffectStore {
    pub fn open(path: &Path) -> Result<Self, FileEffectStoreError> {
        Self::open_with_limits(path, MAX_RETAINED_FILE_EFFECTS, MAX_FILE_EFFECT_LOG_BYTES)
    }

    fn open_with_limits(
        path: &Path,
        max_effects: usize,
        max_bytes: u64,
    ) -> Result<Self, FileEffectStoreError> {
        let parent = path
            .parent()
            .ok_or(FileEffectStoreError::Invalid("effect log has no parent"))?;
        std::fs::create_dir_all(parent).map_err(FileEffectStoreError::Io)?;
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(path).map_err(FileEffectStoreError::Io)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(FileEffectStoreError::Io)?;
        if bytes.len() as u64 > max_bytes {
            return Err(FileEffectStoreError::Capacity);
        }

        // A record is committed only with its newline and sync. Discard a torn final append; a
        // complete malformed record is corruption and fails the physical generation closed.
        let valid_len = if bytes.last().is_none_or(|byte| *byte == b'\n') {
            bytes.len()
        } else {
            bytes
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |index| index + 1)
        };
        if valid_len != bytes.len() {
            file.set_len(valid_len as u64)
                .map_err(FileEffectStoreError::Io)?;
            file.sync_data().map_err(FileEffectStoreError::Io)?;
            bytes.truncate(valid_len);
        }

        let mut records = HashMap::<String, EffectRecord>::new();
        for line in bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let event: EffectEvent = serde_json::from_slice(line)
                .map_err(|_| FileEffectStoreError::Corrupt("invalid complete JSON record"))?;
            match event {
                EffectEvent::Intent { identity } => {
                    validate_identity(&identity)?;
                    if records.contains_key(&identity.operation_id) {
                        return Err(FileEffectStoreError::Corrupt(
                            "one operation has multiple intents",
                        ));
                    }
                    records.insert(
                        identity.operation_id.clone(),
                        EffectRecord {
                            identity,
                            result: None,
                            claim_available: false,
                        },
                    );
                }
                EffectEvent::Complete { identity, result } => {
                    validate_identity(&identity)?;
                    validate_result(&identity, &result)?;
                    let record = records.get_mut(&identity.operation_id).ok_or(
                        FileEffectStoreError::Corrupt("completion has no durable intent"),
                    )?;
                    if record.identity != identity || record.result.is_some() {
                        return Err(FileEffectStoreError::Corrupt(
                            "completion conflicts with retained intent",
                        ));
                    }
                    record.result = Some(*result);
                }
            }
        }
        if records.len() > max_effects {
            return Err(FileEffectStoreError::Capacity);
        }
        let incomplete = records
            .values()
            .filter(|record| record.result.is_none())
            .count() as u64;
        let reserved_result_bytes = incomplete
            .checked_mul(RESULT_RESERVATION_BYTES)
            .ok_or(FileEffectStoreError::Capacity)?;
        if (valid_len as u64)
            .checked_add(reserved_result_bytes)
            .is_none_or(|projected| projected > max_bytes)
        {
            return Err(FileEffectStoreError::Capacity);
        }
        file.seek(SeekFrom::End(0))
            .map_err(FileEffectStoreError::Io)?;

        Ok(Self {
            records: Mutex::new(records),
            writer: Mutex::new(EffectWriter {
                file,
                bytes: valid_len as u64,
                reserved_result_bytes,
                dirty: false,
                max_bytes,
            }),
            max_effects,
        })
    }

    pub fn reserve(
        &self,
        identity: &FileEffectIdentity,
    ) -> Result<EffectReservation, FileEffectStoreError> {
        validate_identity(identity)?;
        let mut writer = self.writer()?;
        let mut records = self.records()?;
        if let Some(record) = records.get(&identity.operation_id) {
            if record.identity != *identity {
                return Err(FileEffectStoreError::Conflict);
            }
            return match record.result.as_ref() {
                Some(result) => Ok(EffectReservation::Replay(Box::new(mark_replayed(
                    result.clone(),
                )))),
                None if record.claim_available => Ok(EffectReservation::New),
                None => Err(FileEffectStoreError::Ambiguous),
            };
        }
        if records.len() >= self.max_effects {
            return Err(FileEffectStoreError::Capacity);
        }
        let event = EffectEvent::Intent {
            identity: identity.clone(),
        };
        let encoded = encode_event(&event)?;
        let projected = writer
            .bytes
            .checked_add(writer.reserved_result_bytes)
            .and_then(|bytes| bytes.checked_add(encoded.len() as u64))
            .and_then(|bytes| bytes.checked_add(RESULT_RESERVATION_BYTES))
            .ok_or(FileEffectStoreError::Capacity)?;
        if projected > writer.max_bytes {
            return Err(FileEffectStoreError::Capacity);
        }
        append(&mut writer, &encoded)?;
        writer.reserved_result_bytes += RESULT_RESERVATION_BYTES;
        records.insert(
            identity.operation_id.clone(),
            EffectRecord {
                identity: identity.clone(),
                result: None,
                claim_available: true,
            },
        );
        sync(&mut writer)?;
        Ok(EffectReservation::New)
    }

    /// Consumes the process-local pre-effect admission. No durable claim event is written: after
    /// a process restart, the already-durable intent is therefore Unknown rather than replayed.
    pub fn claim(
        &self,
        identity: &FileEffectIdentity,
    ) -> Result<EffectReservation, FileEffectStoreError> {
        validate_identity(identity)?;
        let _writer = self.writer()?;
        let mut records = self.records()?;
        let record = records
            .get_mut(&identity.operation_id)
            .ok_or(FileEffectStoreError::Ambiguous)?;
        if record.identity != *identity {
            return Err(FileEffectStoreError::Conflict);
        }
        if let Some(result) = record.result.as_ref() {
            return Ok(EffectReservation::Replay(Box::new(mark_replayed(
                result.clone(),
            ))));
        }
        if !record.claim_available {
            return Err(FileEffectStoreError::Ambiguous);
        }
        record.claim_available = false;
        Ok(EffectReservation::New)
    }

    pub fn complete(
        &self,
        identity: &FileEffectIdentity,
        result: FileEffectStoredResult,
    ) -> Result<FileEffectStoredResult, FileEffectStoreError> {
        validate_identity(identity)?;
        validate_result(identity, &result)?;
        let mut writer = self.writer()?;
        let mut records = self.records()?;
        let record = records
            .get_mut(&identity.operation_id)
            .ok_or(FileEffectStoreError::Ambiguous)?;
        if record.identity != *identity {
            return Err(FileEffectStoreError::Conflict);
        }
        if let Some(existing) = record.result.as_ref() {
            if canonical_bytes(existing)? != canonical_bytes(&result)? {
                return Err(FileEffectStoreError::Conflict);
            }
            return Ok(mark_replayed(existing.clone()));
        }
        if record.claim_available {
            return Err(FileEffectStoreError::Invalid(
                "effect completion has no consumed claim",
            ));
        }
        let event = EffectEvent::Complete {
            identity: identity.clone(),
            result: Box::new(result.clone()),
        };
        let encoded = encode_event(&event)?;
        if encoded.len() as u64 > RESULT_RESERVATION_BYTES {
            return Err(FileEffectStoreError::Capacity);
        }
        writer.reserved_result_bytes = writer
            .reserved_result_bytes
            .checked_sub(RESULT_RESERVATION_BYTES)
            .ok_or(FileEffectStoreError::Corrupt(
                "completion has no result reservation",
            ))?;
        if let Err(error) = append(&mut writer, &encoded) {
            writer.reserved_result_bytes += RESULT_RESERVATION_BYTES;
            return Err(error);
        }
        record.result = Some(result.clone());
        sync(&mut writer)?;
        Ok(result)
    }

    fn writer(&self) -> Result<std::sync::MutexGuard<'_, EffectWriter>, FileEffectStoreError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| FileEffectStoreError::Corrupt("effect writer lock is poisoned"))?;
        if writer.dirty {
            sync(&mut writer)?;
        }
        Ok(writer)
    }

    fn records(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, EffectRecord>>, FileEffectStoreError>
    {
        self.records
            .lock()
            .map_err(|_| FileEffectStoreError::Corrupt("effect index lock is poisoned"))
    }
}

fn encode_event(event: &EffectEvent) -> Result<Vec<u8>, FileEffectStoreError> {
    let mut encoded = serde_json::to_vec(event)
        .map_err(|_| FileEffectStoreError::Invalid("effect event is not serializable"))?;
    encoded.push(b'\n');
    Ok(encoded)
}

fn append(writer: &mut EffectWriter, encoded: &[u8]) -> Result<(), FileEffectStoreError> {
    let next = writer
        .bytes
        .checked_add(encoded.len() as u64)
        .ok_or(FileEffectStoreError::Capacity)?;
    if next
        .checked_add(writer.reserved_result_bytes)
        .is_none_or(|projected| projected > writer.max_bytes)
    {
        return Err(FileEffectStoreError::Capacity);
    }
    let start = writer.bytes;
    writer
        .file
        .seek(SeekFrom::Start(start))
        .map_err(FileEffectStoreError::Io)?;
    if let Err(error) = writer.file.write_all(encoded) {
        let _ = writer.file.set_len(start);
        let _ = writer.file.seek(SeekFrom::Start(start));
        return Err(FileEffectStoreError::Io(error));
    }
    writer.bytes = next;
    writer.dirty = true;
    Ok(())
}

fn sync(writer: &mut EffectWriter) -> Result<(), FileEffectStoreError> {
    writer.file.sync_data().map_err(FileEffectStoreError::Io)?;
    writer.dirty = false;
    Ok(())
}

fn validate_identity(identity: &FileEffectIdentity) -> Result<(), FileEffectStoreError> {
    identity
        .operation_id
        .parse::<environment_wire::Identifier>()
        .map_err(|_| FileEffectStoreError::Invalid("operation_id"))?;
    identity
        .request_digest
        .parse::<environment_wire::Digest>()
        .map_err(|_| FileEffectStoreError::Invalid("request_digest"))?;
    Ok(())
}

fn validate_result(
    identity: &FileEffectIdentity,
    result: &FileEffectStoredResult,
) -> Result<(), FileEffectStoreError> {
    let exact = match result {
        FileEffectStoredResult::Write(result) => {
            identity.kind == FileEffectKind::Write
                && result.operation_id.as_str() == identity.operation_id
                && result.request_digest.as_str() == identity.request_digest
        }
        FileEffectStoredResult::Copy(result) => {
            matches!(
                identity.kind,
                FileEffectKind::CopyImport | FileEffectKind::CopyExport
            ) && result.operation_id.as_str() == identity.operation_id
                && result.request_digest.as_str() == identity.request_digest
        }
    };
    if exact {
        Ok(())
    } else {
        Err(FileEffectStoreError::Invalid("result identity"))
    }
}

fn canonical_bytes(result: &FileEffectStoredResult) -> Result<Vec<u8>, FileEffectStoreError> {
    serde_jcs::to_vec(result)
        .map_err(|_| FileEffectStoreError::Invalid("result is not canonicalizable"))
}

fn mark_replayed(mut result: FileEffectStoredResult) -> FileEffectStoredResult {
    match &mut result {
        FileEffectStoredResult::Write(result) => result.replayed = true,
        FileEffectStoredResult::Copy(result) => result.replayed = true,
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(request: char, kind: FileEffectKind) -> FileEffectIdentity {
        FileEffectIdentity {
            kind,
            operation_id: "file-operation-1".into(),
            request_digest: request.to_string().repeat(64),
        }
    }

    fn write_result(request: char) -> FileEffectStoredResult {
        FileEffectStoredResult::Write(
            serde_json::from_value(serde_json::json!({
                "operation_id": "file-operation-1",
                "request_digest": request.to_string().repeat(64),
                "replayed": false,
                "file": {
                    "path": "result.txt",
                    "kind": "file",
                    "bytes": 2,
                    "sha256": "c".repeat(64),
                    "modified_at_ms": 1
                }
            }))
            .unwrap(),
        )
    }

    fn copy_result(request: char) -> FileEffectStoredResult {
        FileEffectStoredResult::Copy(
            serde_json::from_value(serde_json::json!({
                "operation_id": "file-operation-1",
                "request_digest": request.to_string().repeat(64),
                "replayed": false,
                "file": {
                    "path": "result.txt",
                    "kind": "file",
                    "bytes": 2,
                    "sha256": "c".repeat(64),
                    "modified_at_ms": 1
                },
                "object": {
                    "bytes": 2,
                    "object_id": "object-result-1",
                    "sha256": "d".repeat(64)
                }
            }))
            .unwrap(),
        )
    }

    #[test]
    fn lost_success_result_replays_after_restart_and_changed_digest_conflicts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file-effects.jsonl");
        let exact = identity('a', FileEffectKind::Write);
        {
            let store = FileEffectStore::open(&path).unwrap();
            assert!(matches!(
                store.reserve(&exact).unwrap(),
                EffectReservation::New
            ));
            assert!(matches!(
                store.claim(&exact).unwrap(),
                EffectReservation::New
            ));
            store.complete(&exact, write_result('a')).unwrap();
        }
        let store = FileEffectStore::open(&path).unwrap();
        let EffectReservation::Replay(result) = store.reserve(&exact).unwrap() else {
            panic!("exact result must replay");
        };
        let FileEffectStoredResult::Write(result) = *result else {
            panic!("write intent must replay a write result");
        };
        assert!(result.replayed);
        assert!(matches!(
            store.reserve(&identity('b', FileEffectKind::Write)),
            Err(FileEffectStoreError::Conflict)
        ));
        assert!(matches!(
            store.reserve(&identity('a', FileEffectKind::CopyImport)),
            Err(FileEffectStoreError::Conflict)
        ));
    }

    #[test]
    fn restart_after_intent_never_repeats_the_unknown_effect() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file-effects.jsonl");
        let exact = identity('a', FileEffectKind::Write);
        FileEffectStore::open(&path)
            .unwrap()
            .reserve(&exact)
            .unwrap();
        let store = FileEffectStore::open(&path).unwrap();
        assert!(matches!(
            store.reserve(&exact),
            Err(FileEffectStoreError::Ambiguous)
        ));
    }

    #[test]
    fn lost_copy_success_replays_after_restart_without_authority_material() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file-effects.jsonl");
        let exact = identity('a', FileEffectKind::CopyExport);
        {
            let store = FileEffectStore::open(&path).unwrap();
            assert!(matches!(
                store.reserve(&exact).unwrap(),
                EffectReservation::New
            ));
            assert!(matches!(
                store.claim(&exact).unwrap(),
                EffectReservation::New
            ));
            store.complete(&exact, copy_result('a')).unwrap();
        }
        let log = std::fs::read_to_string(&path).unwrap();
        assert!(!log.contains("url"));
        assert!(!log.contains("headers"));
        assert!(!log.contains("transfer_id"));

        let store = FileEffectStore::open(&path).unwrap();
        let EffectReservation::Replay(result) = store.reserve(&exact).unwrap() else {
            panic!("exact copy result must replay");
        };
        let FileEffectStoredResult::Copy(result) = *result else {
            panic!("copy intent must replay a copy result");
        };
        assert!(result.replayed);
        assert_eq!(result.object.unwrap().object_id.as_str(), "object-result-1");
        assert!(matches!(
            store.reserve(&identity('b', FileEffectKind::CopyExport)),
            Err(FileEffectStoreError::Conflict)
        ));
    }

    #[test]
    fn lost_reservation_response_replays_admission_but_claim_is_single_use() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file-effects.jsonl");
        let exact = identity('a', FileEffectKind::Write);
        let store = FileEffectStore::open(&path).unwrap();
        assert!(matches!(
            store.reserve(&exact).unwrap(),
            EffectReservation::New
        ));
        assert!(matches!(
            store.reserve(&exact).unwrap(),
            EffectReservation::New
        ));
        assert!(matches!(
            store.claim(&exact).unwrap(),
            EffectReservation::New
        ));
        assert!(matches!(
            store.claim(&exact),
            Err(FileEffectStoreError::Ambiguous)
        ));
    }
}
