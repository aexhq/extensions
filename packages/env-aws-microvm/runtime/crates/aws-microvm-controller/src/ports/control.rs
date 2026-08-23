//! `SandboxControlPort`: sandbox execution and stdin control.

use crate::*;

#[async_trait]
impl SandboxControlPort for AwsMicrovmEnvironment {
    async fn create(&self, request: CreateSandboxRequest) -> EnvironmentResult<SandboxStatus> {
        require_additional_target(&request.target)?;
        let preparation = self.preparation(request.target.session_id.as_str()).await?;
        if preparation.request.root_id != request.target.root_id {
            return Err(binding_error(
                "additional sandbox target does not belong to the prepared root",
            ));
        }
        validate_resource_ceiling_subset(&request.resources, &preparation.request.resources)?;
        if !network_ceiling_is_subset(&request.network, &preparation.request.network) {
            return Err(error(
                EnvironmentErrorCode::GenerationConflict,
                false,
                "additional sandbox network policy widens the immutable root seal",
            ));
        }
        let key = target_key(&request.target)?;
        let installed = self
            .materialize(
                key,
                request.target.session_id.as_str(),
                &request.resources,
                &request.network,
                request.resource_class.as_str(),
                MaterializationMode::Additional(request.generation_intent.as_str()),
            )
            .await?;
        Ok(running_status(request.target, &installed))
    }

    async fn inspect(&self, target: SandboxTarget) -> EnvironmentResult<SandboxStatus> {
        require_additional_target(&target)?;
        SandboxFilesPort::status(self, target).await
    }

    async fn execute(&self, request: SandboxExecutionRequest) -> EnvironmentResult<SubmitReceipt> {
        require_additional_target(&request.target)?;
        if sandbox_execution_request_digest(&request) != request.request_digest {
            return Err(invalid("sandbox execution request_digest is not canonical"));
        }
        let installed = self
            .resolve_target(&request.target, Some(request.expected_generation.as_str()))
            .await?;
        match self
            .guest_rpc(&installed, RequestCall::ExecuteSandbox(request))
            .await?
        {
            ResponseReply::ExecuteSandbox(receipt) => Ok(receipt),
            _ => Err(wrong_reply("execute")),
        }
    }

    async fn write_stdin(&self, request: WriteStdinRequest) -> EnvironmentResult<WriteStdinReceipt> {
        require_additional_target(&request.target)?;
        if write_stdin_request_digest(&request) != request.request_digest {
            return Err(invalid("write_stdin request_digest is not canonical"));
        }
        if request.text.len() > brain_protocol::MAX_WRITE_STDIN_BYTES {
            return Err(invalid(format!(
                "write_stdin text exceeds the {}-byte atomic bound",
                brain_protocol::MAX_WRITE_STDIN_BYTES
            )));
        }
        let installed = self
            .resolve_target(&request.target, Some(request.expected_generation.as_str()))
            .await?;
        match self
            .guest_rpc(&installed, RequestCall::WriteStdin(request))
            .await?
        {
            ResponseReply::WriteStdin(receipt) => Ok(receipt),
            _ => Err(wrong_reply("stdin")),
        }
    }

    async fn terminate(&self, target: SandboxTarget) -> EnvironmentResult<SandboxStatus> {
        require_additional_target(&target)?;
        let installed = self.resolve_target(&target, None).await?;
        self.terminate_target(&installed, "explicit additional lifecycle operation")
            .await?;
        Ok(terminated_status(
            target,
            &installed,
            "explicit additional lifecycle operation",
        ))
    }
}
