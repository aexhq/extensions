//! `SandboxFilesPort`: live file operations and file-effect projection helpers.

use crate::*;

#[async_trait]
impl SandboxFilesPort for AwsMicrovmEnvironment {
    async fn status(&self, target: SandboxTarget) -> EnvironmentResult<SandboxStatus> {
        let key = target_key(&target)?;
        let record = self
            .plane
            .registry
            .get(&key)
            .await
            .map_err(materialization_error)?;
        let Some(record) = record else {
            return Ok(status_from_record(target, None));
        };
        let Some(installed) = record.installed() else {
            return Ok(status_from_record(target, Some(record)));
        };
        if now_ms() >= installed.expires_at_ms {
            let reason = "physical target hard deadline reached";
            // Hard lifetime expiry is a confirmed physical loss, not an explicit logical
            // termination. A default target may therefore get a fresh generation later, while an
            // additional target remains fenced by its durable Gone tombstone.
            self.confirm_provider_termination(&installed).await?;
            self.record_gone(&installed, reason).await?;
            return Ok(gone_status(target, &installed, reason));
        }
        match self.plane.control.get(&installed.target_ref).await {
            Ok(vm) if is_terminated(&vm.state) => {
                let reason = "provider reports physical generation gone";
                self.record_gone(&installed, reason).await?;
                Ok(gone_status(target, &installed, reason))
            }
            Ok(vm) => {
                let mut status = status_from_record(target, Some(record));
                // The provider exposes only the state observed by this GetMicrovm call. It does
                // not expose when auto-suspend occurred, so preserve the durable registry
                // timestamp rather than fabricating a suspension transition timestamp.
                status.state = sandbox_state_from_provider(&vm.state)?;
                Ok(status)
            }
            Err(ControlError::Gone(_)) => {
                let reason = "provider reports physical generation gone";
                self.record_gone(&installed, reason).await?;
                Ok(gone_status(target, &installed, reason))
            }
            Err(error_value) => Err(control_error(error_value)),
        }
    }

    async fn list(&self, request: SandboxFileListRequest) -> EnvironmentResult<SandboxFileList> {
        let installed = self
            .resolve_target(&request.target, Some(&request.expected_generation))
            .await?;
        match self
            .guest_rpc(&installed, RequestCall::ListFiles(request))
            .await?
        {
            ResponseReply::ListFiles(value) => Ok(value),
            _ => Err(wrong_reply("list")),
        }
    }

    async fn stat(&self, request: SandboxFileRequest) -> EnvironmentResult<FileEntry> {
        let installed = self
            .resolve_target(&request.target, Some(request.expected_generation.as_str()))
            .await?;
        match self
            .guest_rpc(&installed, RequestCall::StatFile(request))
            .await?
        {
            ResponseReply::StatFile(value) => Ok(value),
            _ => Err(wrong_reply("stat")),
        }
    }

    async fn read(&self, request: SandboxFileRequest) -> EnvironmentResult<SandboxFileContent> {
        let installed = self
            .resolve_target(&request.target, Some(request.expected_generation.as_str()))
            .await?;
        match self
            .guest_rpc(&installed, RequestCall::ReadFile(request))
            .await?
        {
            ResponseReply::ReadFile(value) => Ok(value),
            _ => Err(wrong_reply("read")),
        }
    }

    async fn write(&self, request: SandboxFileWriteRequest) -> EnvironmentResult<SandboxFileWriteResult> {
        if sandbox_file_write_request_digest(&request) != request.request_digest {
            return Err(invalid(
                "sandbox file write request_digest is not canonical",
            ));
        }
        let lock = file_effect_lock_index(request.operation_id.as_str());
        let _guard = self.file_effect_locks[lock].lock().await;
        let installed = self
            .resolve_target(&request.target, Some(request.expected_generation.as_str()))
            .await?;
        let identity = FileEffectIdentity {
            kind: FileEffectKind::Write,
            operation_id: request.operation_id.to_string(),
            request_digest: request.request_digest.to_string(),
        };
        match self.reserve_file_effect(&installed, identity).await? {
            FileEffectReservation::Replay(result) => {
                let FileEffectStoredResult::Write(result) = *result else {
                    return Err(temporary("guest replayed the wrong file effect kind"));
                };
                return Ok(result);
            }
            FileEffectReservation::New => {}
        }
        if let SandboxFileWriteSource::Object { object, fetch } = &request.source {
            let staged = fetch_object(self.plane.guest.http(), fetch, object).await?;
            let result = self.install_object(&installed, object, &staged).await;
            self.settle_guest_result(&installed, result).await?;
            // The guest receives the original immutable reference and finds the verified staged
            // bytes by digest. The one-purpose authority is never dereferenced by untrusted code.
        }
        match self
            .guest_rpc(
                &installed,
                RequestCall::WriteFile(project_guest_file_write(request)),
            )
            .await?
        {
            ResponseReply::WriteFile(FileEffectStoredResult::Write(value)) => Ok(value),
            _ => Err(wrong_reply("write")),
        }
    }

    async fn find(&self, request: SandboxSearchRequest) -> EnvironmentResult<SandboxFileList> {
        let installed = self
            .resolve_target(&request.target, Some(&request.expected_generation))
            .await?;
        match self
            .guest_rpc(&installed, RequestCall::FindFiles(request))
            .await?
        {
            ResponseReply::FindFiles(value) => Ok(value),
            _ => Err(wrong_reply("find")),
        }
    }

    async fn grep(&self, request: SandboxSearchRequest) -> EnvironmentResult<SandboxFileList> {
        let installed = self
            .resolve_target(&request.target, Some(&request.expected_generation))
            .await?;
        match self
            .guest_rpc(&installed, RequestCall::GrepFiles(request))
            .await?
        {
            ResponseReply::GrepFiles(value) => Ok(value),
            _ => Err(wrong_reply("grep")),
        }
    }

    async fn transfer(&self, request: SandboxCopyRequest) -> EnvironmentResult<SandboxCopyResult> {
        if sandbox_copy_request_digest(&request) != request.request_digest {
            return Err(invalid("sandbox copy request_digest is not canonical"));
        }
        let lock = file_effect_lock_index(request.operation_id.as_str());
        let _guard = self.file_effect_locks[lock].lock().await;
        let installed = self
            .resolve_target(&request.target, Some(request.expected_generation.as_str()))
            .await?;
        let effect_kind = match request.direction {
            SandboxCopyRequestDirection::Import => FileEffectKind::CopyImport,
            SandboxCopyRequestDirection::Export => FileEffectKind::CopyExport,
        };
        let identity = file_effect_identity(&request, effect_kind);
        match self
            .reserve_file_effect(&installed, identity.clone())
            .await?
        {
            FileEffectReservation::Replay(result) => {
                let FileEffectStoredResult::Copy(result) = *result else {
                    return Err(temporary("guest replayed the wrong copy effect kind"));
                };
                return Ok(result);
            }
            FileEffectReservation::New => {}
        }
        match request.direction {
            SandboxCopyRequestDirection::Import => {
                self.transfer_import(request, &installed, identity).await
            }
            SandboxCopyRequestDirection::Export => {
                self.transfer_export(request, &installed, identity).await
            }
        }
    }
}

impl AwsMicrovmEnvironment {
    /// Import: consume the GET authority host-side, install the staged object into the guest,
    /// and let the guest perform the two-phase file effect against the installed object.
    async fn transfer_import(
        &self,
        request: SandboxCopyRequest,
        installed: &InstalledTarget,
        identity: FileEffectIdentity,
    ) -> EnvironmentResult<SandboxCopyResult> {
        let object = request
            .object
            .as_ref()
            .ok_or_else(|| invalid("import requires an object reference"))?;
        let staged = fetch_object(self.plane.guest.http(), &request.transfer, object).await?;
        let result = self.install_object(installed, object, &staged).await;
        self.settle_guest_result(installed, result).await?;
        let write = GuestFileWriteRequest {
            effect: identity,
            expected_generation: request.expected_generation.to_string(),
            overwrite: request.overwrite,
            path: request.path.to_string(),
            source: GuestFileWriteSource::InstalledObject {
                object: object.clone(),
            },
            target: request.target,
        };
        match self
            .guest_rpc(installed, RequestCall::WriteFile(write))
            .await?
        {
            ResponseReply::WriteFile(FileEffectStoredResult::Copy(result)) => Ok(result),
            _ => Err(wrong_reply("import")),
        }
    }

    /// Export: stream the live file out of the guest, stage it host-side, consume the PUT
    /// authority, and record the exact streamed snapshot identity as the effect result.
    async fn transfer_export(
        &self,
        request: SandboxCopyRequest,
        installed: &InstalledTarget,
        identity: FileEffectIdentity,
    ) -> EnvironmentResult<SandboxCopyResult> {
        let read = SandboxFileRequest {
            expected_generation: request.expected_generation.clone(),
            path: request
                .path
                .as_str()
                .parse()
                .map_err(|_| invalid("export path cannot be projected"))?,
            target: request.target.clone(),
        };
        validate_transfer_authority(&request.transfer, ObjectTransferAuthorityMethod::Put, 0)?;
        let result = self.plane.guest.export_file(installed, &read).await;
        let (mut file, response) = self.settle_guest_result(installed, result).await?;
        let staged = stage_response(
            response,
            request.transfer.max_bytes.get().min(MAX_OBJECT_BYTES),
            request.transfer.expires_at_ms.get(),
        )
        .await?;
        match self.claim_file_effect(installed, identity).await? {
            FileEffectReservation::Replay(result) => {
                let FileEffectStoredResult::Copy(result) = *result else {
                    return Err(temporary("guest replayed the wrong copy claim kind"));
                };
                return Ok(result);
            }
            FileEffectReservation::New => {}
        }
        put_object(self.plane.guest.http(), &request.transfer, &staged).await?;
        // The Tool may mutate an open file while it is copied. Publish the exact streamed
        // snapshot identity, not stale pre-stream metadata from the opened path.
        file.bytes = staged.bytes;
        file.sha256 = Some(staged.sha256.parse().expect("digest"));
        let object = ObjectReference {
            bytes: staged.bytes,
            media_type: None,
            object_id: request.transfer.object_id.clone(),
            sha256: staged.sha256.parse().expect("digest"),
        };
        let result = SandboxCopyResult {
            file,
            object: Some(object),
            operation_id: request.operation_id,
            replayed: false,
            request_digest: request.request_digest,
        };
        match self
            .guest_rpc(
                installed,
                RequestCall::CompleteFileEffect(FileEffectStoredResult::Copy(result)),
            )
            .await?
        {
            ResponseReply::CompleteFileEffect(FileEffectStoredResult::Copy(result)) => Ok(result),
            _ => Err(wrong_reply("copy completion")),
        }
    }
}

pub(crate) fn require_additional_target(target: &SandboxTarget) -> EnvironmentResult<()> {
    if target.kind != TargetKind::Additional || target.sandbox_id.is_none() {
        return Err(invalid("additional sandbox target is required"));
    }
    Ok(())
}

pub(crate) fn project_guest_file_write(request: SandboxFileWriteRequest) -> GuestFileWriteRequest {
    GuestFileWriteRequest {
        effect: FileEffectIdentity {
            kind: FileEffectKind::Write,
            operation_id: request.operation_id.to_string(),
            request_digest: request.request_digest.to_string(),
        },
        expected_generation: request.expected_generation.to_string(),
        overwrite: request.overwrite,
        path: request.path.to_string(),
        source: match request.source {
            SandboxFileWriteSource::Inline { content_base64 } => GuestFileWriteSource::Inline {
                content_base64: content_base64.to_string(),
            },
            SandboxFileWriteSource::Object { object, .. } => {
                GuestFileWriteSource::InstalledObject { object }
            }
        },
        target: request.target,
    }
}

pub(crate) fn file_effect_identity(
    request: &SandboxCopyRequest,
    kind: FileEffectKind,
) -> FileEffectIdentity {
    FileEffectIdentity {
        kind,
        operation_id: request.operation_id.to_string(),
        request_digest: request.request_digest.to_string(),
    }
}

pub(crate) fn file_effect_lock_index(operation_id: &str) -> usize {
    shard_index(&[operation_id], FILE_EFFECT_LOCK_SHARDS)
}
