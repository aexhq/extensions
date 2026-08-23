//! Live workspace file operations and two-phase file effects.

use super::*;

impl Environment {
    pub(crate) fn workspace_files(&self) -> Result<LiveFiles, EnvironmentError> {
        self.files
            .try_clone()
            .map_err(|_| unavailable("workspace capability cannot be cloned"))
    }

    pub async fn list_files(
        &self,
        request: SandboxFileListRequest,
    ) -> Result<SandboxFileList, EnvironmentError> {
        self.fence(&request.target, &request.expected_generation)
            .await?;
        let files = self.workspace_files()?;
        let path = request.path.to_string();
        let cursor = request.cursor.map(|cursor| cursor.to_string());
        let limit = request.limit as usize;
        let page = blocking_file(move || files.list(&path, cursor.as_deref(), limit)).await?;
        Ok(SandboxFileList {
            entries: page
                .entries
                .iter()
                .map(file_entry)
                .collect::<Result<_, _>>()?,
            next_cursor: page.next_cursor,
        })
    }

    pub async fn stat_file(
        &self,
        request: SandboxFileRequest,
    ) -> Result<FileEntry, EnvironmentError> {
        self.fence(&request.target, request.expected_generation.as_str())
            .await?;
        let files = self.workspace_files()?;
        let path = request.path.to_string();
        let entry = blocking_file(move || files.stat(&path)).await?;
        file_entry(&entry)
    }

    pub async fn read_file(
        &self,
        request: SandboxFileRequest,
    ) -> Result<SandboxFileContent, EnvironmentError> {
        self.fence(&request.target, request.expected_generation.as_str())
            .await?;
        let files = self.workspace_files()?;
        let path = request.path.to_string();
        let content = blocking_file(move || files.read(&path, MAX_INLINE_FILE_BYTES)).await?;
        Ok(SandboxFileContent {
            entry: file_entry(&content.entry)?,
            content_base64: base64::engine::general_purpose::STANDARD.encode(content.bytes),
        })
    }

    pub async fn write_file(
        &self,
        request: GuestFileWriteRequest,
    ) -> Result<FileEffectStoredResult, EnvironmentError> {
        self.fence(&request.target, request.expected_generation.as_str())
            .await?;
        if request.effect.kind == FileEffectKind::CopyExport {
            return Err(invalid("copy export cannot use the workspace write path"));
        }
        let lock = file_effect_lock_index(&request.effect.operation_id);
        let _guard = self.effects.locks[lock].lock().await;
        match self
            .claim_file_effect_unlocked(request.effect.clone())
            .await?
        {
            FileEffectReservation::Replay(result) => return Ok(*result),
            FileEffectReservation::New => {}
        }
        let files = self.workspace_files()?;
        let path = request.path.to_string();
        let overwrite = request.overwrite;
        let entry = match request.source {
            GuestFileWriteSource::Inline { content_base64 } => {
                blocking_environment(move || {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(content_base64.as_bytes())
                        .map_err(|_| invalid("inline file content is not padded base64"))?;
                    if bytes.len() > 1024 * 1024 {
                        return Err(invalid("inline file content exceeds 1 MiB"));
                    }
                    files.write(&path, &bytes, overwrite).map_err(file_error)
                })
                .await?
            }
            GuestFileWriteSource::InstalledObject { object } => {
                let source = self.cfg.object_dir.join(object.sha256.as_str());
                let bytes = object.bytes;
                let digest = object.sha256.to_string();
                blocking_environment(move || {
                    if !source.is_file() {
                        return Err(invalid(
                            "object file write was not staged by the trusted Environment adapter",
                        ));
                    }
                    files
                        .write_from_file(&path, &source, bytes, &digest, overwrite)
                        .map_err(file_error)
                })
                .await?
            }
        };
        let file = file_entry(&entry)?;
        let result = match request.effect.kind {
            FileEffectKind::Write => FileEffectStoredResult::Write(SandboxFileWriteResult {
                file,
                operation_id: request
                    .effect
                    .operation_id
                    .parse()
                    .map_err(|_| invalid("file operation_id is invalid"))?,
                replayed: false,
                request_digest: request
                    .effect
                    .request_digest
                    .parse()
                    .map_err(|_| invalid("file request_digest is invalid"))?,
            }),
            FileEffectKind::CopyImport => FileEffectStoredResult::Copy(SandboxCopyResult {
                file,
                object: None,
                operation_id: request
                    .effect
                    .operation_id
                    .parse()
                    .map_err(|_| invalid("copy operation_id is invalid"))?,
                replayed: false,
                request_digest: request
                    .effect
                    .request_digest
                    .parse()
                    .map_err(|_| invalid("copy request_digest is invalid"))?,
            }),
            FileEffectKind::CopyExport => unreachable!("checked above"),
        };
        self.complete_file_effect_inner(request.effect, result)
            .await
    }

    /// Every public file-effect entry point serializes on the per-operation shard lock, so the
    /// socket-driven reserve/claim/complete sequence and the in-process write path can never
    /// interleave on one operation.
    pub async fn reserve_file_effect(
        &self,
        identity: FileEffectIdentity,
    ) -> Result<FileEffectReservation, EnvironmentError> {
        let lock = file_effect_lock_index(&identity.operation_id);
        let _guard = self.effects.locks[lock].lock().await;
        self.reserve_file_effect_unlocked(identity).await
    }

    async fn reserve_file_effect_unlocked(
        &self,
        identity: FileEffectIdentity,
    ) -> Result<FileEffectReservation, EnvironmentError> {
        let store = self.effects.store.clone();
        blocking_environment(move || {
            store
                .reserve(&identity)
                .map(|reservation| match reservation {
                    EffectReservation::New => FileEffectReservation::New,
                    EffectReservation::Replay(result) => FileEffectReservation::Replay(result),
                })
                .map_err(file_effect_store_error)
        })
        .await
    }

    pub async fn claim_file_effect(
        &self,
        identity: FileEffectIdentity,
    ) -> Result<FileEffectReservation, EnvironmentError> {
        let lock = file_effect_lock_index(&identity.operation_id);
        let _guard = self.effects.locks[lock].lock().await;
        self.claim_file_effect_unlocked(identity).await
    }

    async fn claim_file_effect_unlocked(
        &self,
        identity: FileEffectIdentity,
    ) -> Result<FileEffectReservation, EnvironmentError> {
        let store = self.effects.store.clone();
        blocking_environment(move || {
            store
                .claim(&identity)
                .map(|reservation| match reservation {
                    EffectReservation::New => FileEffectReservation::New,
                    EffectReservation::Replay(result) => FileEffectReservation::Replay(result),
                })
                .map_err(file_effect_store_error)
        })
        .await
    }

    pub async fn complete_file_effect(
        &self,
        result: FileEffectStoredResult,
    ) -> Result<FileEffectStoredResult, EnvironmentError> {
        let identity = file_effect_result_identity(&result)?;
        let lock = file_effect_lock_index(&identity.operation_id);
        let _guard = self.effects.locks[lock].lock().await;
        self.complete_file_effect_inner(identity, result).await
    }

    pub(crate) async fn complete_file_effect_inner(
        &self,
        identity: FileEffectIdentity,
        result: FileEffectStoredResult,
    ) -> Result<FileEffectStoredResult, EnvironmentError> {
        let store = self.effects.store.clone();
        blocking_environment(move || {
            store
                .complete(&identity, result)
                .map_err(file_effect_store_error)
        })
        .await
    }

    pub async fn find_files(
        &self,
        request: SandboxSearchRequest,
    ) -> Result<SandboxFileList, EnvironmentError> {
        self.fence(&request.target, &request.expected_generation)
            .await?;
        let files = self.workspace_files()?;
        let path = request.path.to_string();
        let expression = request.expression.to_string();
        let cursor = request.cursor.map(|cursor| cursor.to_string());
        let limit = request.limit as usize;
        let page =
            blocking_file(move || files.find(&path, &expression, cursor.as_deref(), limit)).await?;
        Ok(SandboxFileList {
            entries: page
                .entries
                .iter()
                .map(file_entry)
                .collect::<Result<_, _>>()?,
            next_cursor: page.next_cursor,
        })
    }

    pub async fn grep_files(
        &self,
        request: SandboxSearchRequest,
    ) -> Result<SandboxFileList, EnvironmentError> {
        self.fence(&request.target, &request.expected_generation)
            .await?;
        let files = self.workspace_files()?;
        let path = request.path.to_string();
        let expression = request.expression.to_string();
        let cursor = request.cursor.map(|cursor| cursor.to_string());
        let limit = request.limit as usize;
        let page =
            blocking_file(move || files.grep(&path, &expression, cursor.as_deref(), limit)).await?;
        Ok(SandboxFileList {
            entries: page
                .entries
                .iter()
                .map(file_entry)
                .collect::<Result<_, _>>()?,
            next_cursor: page.next_cursor,
        })
    }
}
pub(crate) async fn blocking_file<T, F>(work: F) -> Result<T, EnvironmentError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, LiveFileError> + Send + 'static,
{
    blocking_environment(move || work().map_err(file_error)).await
}

pub(crate) fn file_effect_lock_index(operation_id: &str) -> usize {
    let digest = Sha256::digest(operation_id.as_bytes());
    let prefix = u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 prefix"));
    prefix as usize % FILE_EFFECT_LOCK_SHARDS
}

pub(crate) fn file_effect_result_identity(
    result: &FileEffectStoredResult,
) -> Result<FileEffectIdentity, EnvironmentError> {
    let (kind, operation_id, request_digest) = match result {
        FileEffectStoredResult::Write(result) => (
            FileEffectKind::Write,
            result.operation_id.to_string(),
            result.request_digest.to_string(),
        ),
        FileEffectStoredResult::Copy(result) => {
            // Only export is completed as a separate trusted-adapter phase. Import is committed
            // atomically by `write_file` around the workspace mutation.
            (
                FileEffectKind::CopyExport,
                result.operation_id.to_string(),
                result.request_digest.to_string(),
            )
        }
    };
    Ok(FileEffectIdentity {
        kind,
        operation_id,
        request_digest,
    })
}

pub(crate) fn file_error(error: LiveFileError) -> EnvironmentError {
    let code = match error {
        LiveFileError::NotFound => EnvironmentErrorCode::FileNotFound,
        LiveFileError::TooLarge | LiveFileError::SearchBoundExceeded => {
            EnvironmentErrorCode::ResourceExhausted
        }
        LiveFileError::Io(_) => EnvironmentErrorCode::TemporarilyUnavailable,
        _ => EnvironmentErrorCode::InvalidRequest,
    };
    environment_error(
        code,
        matches!(error, LiveFileError::Io(_)),
        error.to_string(),
    )
}

pub(crate) fn file_entry(entry: &LiveFileEntry) -> Result<FileEntry, EnvironmentError> {
    Ok(FileEntry {
        bytes: entry.bytes,
        kind: match entry.kind {
            LiveFileKind::File => FileEntryKind::File,
            LiveFileKind::Directory => FileEntryKind::Directory,
            LiveFileKind::Symlink => FileEntryKind::Symlink,
        },
        modified_at_ms: entry.modified_at_ms,
        path: entry
            .path
            .parse()
            .map_err(|_| invalid("file path is invalid"))?,
        sha256: entry
            .sha256
            .as_deref()
            .map(str::parse::<Digest>)
            .transpose()
            .map_err(|_| invalid("file digest is invalid"))?,
    })
}
