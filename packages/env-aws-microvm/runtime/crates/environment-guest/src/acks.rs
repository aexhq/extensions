//! Durable, payload-free acknowledgement fences for released terminal operations.
//!
//! Brain acknowledges a terminal digest only after committing that exact result. The guest may
//! then release the retained payload, but it must continue to reject a later resubmission of the
//! same operation identity. This append-only log is generation-local and is removed only when the
//! physical target is purged.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Mutex, RwLock};

use environment_wire::canonical_digest;
use environment_wire::{Digest, OperationRef};
use serde::{Deserialize, Serialize};

pub const MAX_ACKNOWLEDGEMENT_LOG_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcknowledgedOperation {
    operation_id: String,
    request_digest: String,
    operation_ref_digest: String,
    terminal_digest: String,
}

impl AcknowledgedOperation {
    fn new(operation: &OperationRef, terminal_digest: &Digest) -> Result<Self, AckStoreError> {
        Ok(Self {
            operation_id: operation.operation_id.to_string(),
            request_digest: operation.request_digest.to_string(),
            operation_ref_digest: canonical_digest(operation)
                .map_err(|_| AckStoreError::Invalid("operation reference is not canonicalizable"))?
                .to_string(),
            terminal_digest: terminal_digest.to_string(),
        })
    }

    fn validate(&self) -> Result<(), AckStoreError> {
        validate_identifier(&self.operation_id)?;
        validate_digest(&self.request_digest)?;
        validate_digest(&self.operation_ref_digest)?;
        validate_digest(&self.terminal_digest)
    }
}

struct AckWriter {
    file: File,
    bytes: u64,
    /// A fully written record is fenced in memory before its sync completes. A later retry first
    /// syncs the complete log; it never releases a result based on an uncertain write.
    dirty: bool,
    max_bytes: u64,
}

/// A tiny generation-local durable fence. Reads are memory-only on the operation hot path; the
/// append and `sync_data` happen only after Brain's terminal journal commit.
pub struct AcknowledgementStore {
    records: RwLock<HashMap<String, AcknowledgedOperation>>,
    writer: Mutex<AckWriter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionFence {
    Clear,
    Acknowledged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckRetention {
    New,
    Replay,
}

#[derive(Debug, thiserror::Error)]
pub enum AckStoreError {
    #[error("acknowledgement identity conflicts with the retained tombstone")]
    Conflict,
    #[error("acknowledgement tombstone capacity is exhausted")]
    Capacity,
    #[error("acknowledgement log is corrupt: {0}")]
    Corrupt(&'static str),
    #[error("invalid acknowledgement identity: {0}")]
    Invalid(&'static str),
    #[error("acknowledgement storage is unavailable")]
    Io(#[source] std::io::Error),
}

impl AcknowledgementStore {
    pub fn open(path: &Path) -> Result<Self, AckStoreError> {
        Self::open_with_limit(path, MAX_ACKNOWLEDGEMENT_LOG_BYTES)
    }

    fn open_with_limit(path: &Path, max_bytes: u64) -> Result<Self, AckStoreError> {
        let parent = path
            .parent()
            .ok_or(AckStoreError::Invalid("acknowledgement log has no parent"))?;
        std::fs::create_dir_all(parent).map_err(AckStoreError::Io)?;

        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(path).map_err(AckStoreError::Io)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(AckStoreError::Io)?;
        if bytes.len() as u64 > max_bytes {
            return Err(AckStoreError::Capacity);
        }

        // An acknowledgement is successful only after the newline and sync. A missing final
        // newline therefore represents an uncommitted tail and is safe to discard on recovery.
        let valid_len = if bytes.last().is_some_and(|byte| *byte == b'\n') {
            bytes.len()
        } else {
            bytes
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |index| index + 1)
        };
        if valid_len != bytes.len() {
            file.set_len(valid_len as u64).map_err(AckStoreError::Io)?;
            file.sync_data().map_err(AckStoreError::Io)?;
            bytes.truncate(valid_len);
        }

        let mut records = HashMap::new();
        for line in bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let record: AcknowledgedOperation = serde_json::from_slice(line)
                .map_err(|_| AckStoreError::Corrupt("invalid complete JSON record"))?;
            record.validate()?;
            match records.get(&record.operation_id) {
                Some(existing) if existing == &record => {}
                Some(_) => {
                    return Err(AckStoreError::Corrupt(
                        "one operation id has conflicting tombstones",
                    ));
                }
                None => {
                    records.insert(record.operation_id.clone(), record);
                }
            }
        }
        file.seek(SeekFrom::End(0)).map_err(AckStoreError::Io)?;

        Ok(Self {
            records: RwLock::new(records),
            writer: Mutex::new(AckWriter {
                file,
                bytes: valid_len as u64,
                dirty: false,
                max_bytes,
            }),
        })
    }

    /// Checks the post-ack fence before any bundle validation, secret fetch, or guest effect.
    pub fn fence_submission(
        &self,
        operation_id: &str,
        request_digest: &str,
    ) -> Result<SubmissionFence, AckStoreError> {
        let records = self
            .records
            .read()
            .map_err(|_| AckStoreError::Corrupt("acknowledgement index lock is poisoned"))?;
        match records.get(operation_id) {
            None => Ok(SubmissionFence::Clear),
            Some(record) if record.request_digest == request_digest => {
                Ok(SubmissionFence::Acknowledged)
            }
            Some(_) => Err(AckStoreError::Conflict),
        }
    }

    /// Returns whether this exact acknowledgement was already retained. A mismatch for an
    /// acknowledged operation is always a permanent conflict.
    pub fn acknowledgement_exists(
        &self,
        operation: &OperationRef,
        terminal_digest: &Digest,
    ) -> Result<bool, AckStoreError> {
        let candidate = AcknowledgedOperation::new(operation, terminal_digest)?;
        // A prior append may have completed while sync_data returned an uncertain error. An exact
        // retry cannot trust the in-memory fence until that complete record is forced durable.
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| AckStoreError::Corrupt("acknowledgement writer lock is poisoned"))?;
        if writer.dirty {
            writer.file.sync_data().map_err(AckStoreError::Io)?;
            writer.dirty = false;
        }
        let records = self
            .records
            .read()
            .map_err(|_| AckStoreError::Corrupt("acknowledgement index lock is poisoned"))?;
        match records.get(&candidate.operation_id) {
            None => Ok(false),
            Some(existing) if existing == &candidate => Ok(true),
            Some(_) => Err(AckStoreError::Conflict),
        }
    }

    /// Durably retains an acknowledgement before its terminal payload is released.
    ///
    /// This method performs file I/O and should be called from a blocking worker.
    pub fn retain(
        &self,
        operation: &OperationRef,
        terminal_digest: &Digest,
    ) -> Result<AckRetention, AckStoreError> {
        let candidate = AcknowledgedOperation::new(operation, terminal_digest)?;
        candidate.validate()?;
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| AckStoreError::Corrupt("acknowledgement writer lock is poisoned"))?;

        // If a prior full write returned an uncertain sync result, force it durable before any
        // acknowledgement (including an exact retry) can release a retained terminal payload.
        if writer.dirty {
            writer.file.sync_data().map_err(AckStoreError::Io)?;
            writer.dirty = false;
        }

        {
            let records = self
                .records
                .read()
                .map_err(|_| AckStoreError::Corrupt("acknowledgement index lock is poisoned"))?;
            if let Some(existing) = records.get(&candidate.operation_id) {
                return if existing == &candidate {
                    Ok(AckRetention::Replay)
                } else {
                    Err(AckStoreError::Conflict)
                };
            }
        }

        let mut encoded = serde_json::to_vec(&candidate)
            .map_err(|_| AckStoreError::Invalid("acknowledgement is not serializable"))?;
        encoded.push(b'\n');
        let next_bytes = writer
            .bytes
            .checked_add(encoded.len() as u64)
            .ok_or(AckStoreError::Capacity)?;
        if next_bytes > writer.max_bytes {
            return Err(AckStoreError::Capacity);
        }

        let start = writer.bytes;
        writer
            .file
            .seek(SeekFrom::Start(start))
            .map_err(AckStoreError::Io)?;
        if let Err(error) = writer.file.write_all(&encoded) {
            // A partial JSON record must never be followed by another record in the same process.
            let _ = writer.file.set_len(start);
            let _ = writer.file.seek(SeekFrom::Start(start));
            return Err(AckStoreError::Io(error));
        }
        writer.bytes = next_bytes;
        writer.dirty = true;
        self.records
            .write()
            .map_err(|_| AckStoreError::Corrupt("acknowledgement index lock is poisoned"))?
            .insert(candidate.operation_id.clone(), candidate);
        writer.file.sync_data().map_err(AckStoreError::Io)?;
        writer.dirty = false;
        Ok(AckRetention::New)
    }
}

fn validate_identifier(value: &str) -> Result<(), AckStoreError> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(AckStoreError::Invalid("operation_id"));
    };
    if value.len() > 128
        || !value.is_ascii()
        || !first.is_ascii_alphanumeric()
        || chars.any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ':' | '-')))
    {
        return Err(AckStoreError::Invalid("operation_id"));
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<(), AckStoreError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AckStoreError::Invalid("digest"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operation(request: char) -> OperationRef {
        serde_json::from_value(serde_json::json!({
            "generation": "generation-1",
            "operation_id": "operation-1",
            "receipt_ref": "receipt-1",
            "request_digest": request.to_string().repeat(64),
            "target": {
                "binding_ref": "binding-1",
                "kind": "environment",
                "root_id": "root-1",
                "session_id": "session-1"
            },
            "target_ref": "target-1"
        }))
        .unwrap()
    }

    #[test]
    fn exact_ack_replays_after_restart_and_conflicts_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("acks.jsonl");
        let terminal: Digest = "c".repeat(64).parse().unwrap();
        {
            let store = AcknowledgementStore::open(&path).unwrap();
            assert_eq!(
                store
                    .fence_submission("operation-1", &"a".repeat(64))
                    .unwrap(),
                SubmissionFence::Clear
            );
            assert_eq!(
                store.retain(&operation('a'), &terminal).unwrap(),
                AckRetention::New
            );
            assert_eq!(
                store.retain(&operation('a'), &terminal).unwrap(),
                AckRetention::Replay
            );
        }

        let store = AcknowledgementStore::open(&path).unwrap();
        assert_eq!(
            store
                .fence_submission("operation-1", &"a".repeat(64))
                .unwrap(),
            SubmissionFence::Acknowledged
        );
        assert!(matches!(
            store.fence_submission("operation-1", &"b".repeat(64)),
            Err(AckStoreError::Conflict)
        ));
        assert!(matches!(
            store.retain(&operation('a'), &"d".repeat(64).parse().unwrap()),
            Err(AckStoreError::Conflict)
        ));
        let mut rerouted = operation('a');
        rerouted.target.root_id = "root-2".parse().unwrap();
        assert!(matches!(
            store.retain(&rerouted, &terminal),
            Err(AckStoreError::Conflict)
        ));
    }

    #[test]
    fn replay_forces_an_uncertain_complete_append_durable_before_success() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("acks.jsonl");
        let terminal: Digest = "c".repeat(64).parse().unwrap();
        let store = AcknowledgementStore::open(&path).unwrap();
        store.retain(&operation('a'), &terminal).unwrap();
        // Model a complete append whose first sync result was uncertain. The exact retry must
        // perform the sync path rather than trusting only the in-memory index.
        store.writer.lock().unwrap().dirty = true;
        assert!(
            store
                .acknowledgement_exists(&operation('a'), &terminal)
                .unwrap()
        );
        assert!(!store.writer.lock().unwrap().dirty);
    }

    #[test]
    fn an_uncommitted_tail_is_discarded_but_complete_corruption_is_fatal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("acks.jsonl");
        let terminal: Digest = "c".repeat(64).parse().unwrap();
        AcknowledgementStore::open(&path)
            .unwrap()
            .retain(&operation('a'), &terminal)
            .unwrap();
        {
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(br#"{"operation_id":"torn""#).unwrap();
        }
        let store = AcknowledgementStore::open(&path).unwrap();
        assert_eq!(
            store
                .fence_submission("operation-1", &"a".repeat(64))
                .unwrap(),
            SubmissionFence::Acknowledged
        );

        std::fs::write(&path, b"not-json\n").unwrap();
        assert!(matches!(
            AcknowledgementStore::open(&path),
            Err(AckStoreError::Corrupt(_))
        ));
    }

    #[test]
    fn bounded_log_refuses_a_new_tombstone_without_losing_the_old_one() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("acks.jsonl");
        let terminal: Digest = "c".repeat(64).parse().unwrap();
        let store = AcknowledgementStore::open_with_limit(&path, 512).unwrap();
        store.retain(&operation('a'), &terminal).unwrap();

        let second: OperationRef = serde_json::from_value(serde_json::json!({
            "generation": "generation-1",
            "operation_id": "operation-2",
            "receipt_ref": "receipt-1",
            "request_digest": "b".repeat(64),
            "target": {
                "binding_ref": "binding-1",
                "kind": "environment",
                "root_id": "root-1",
                "session_id": "session-1"
            },
            "target_ref": "target-1"
        }))
        .unwrap();
        assert!(matches!(
            store.retain(&second, &terminal),
            Err(AckStoreError::Capacity)
        ));
        assert_eq!(
            store
                .fence_submission("operation-1", &"a".repeat(64))
                .unwrap(),
            SubmissionFence::Acknowledged
        );
    }
}
