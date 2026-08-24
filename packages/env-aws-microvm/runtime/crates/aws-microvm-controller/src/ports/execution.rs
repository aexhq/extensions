//! `EnvironmentPort`: operation submit/observe/cancel/acknowledge over the guest channel.

use crate::*;

#[async_trait]
impl EnvironmentPort for AwsMicrovmEnvironment {
    async fn resolve_binding(&self, binding: SealedBinding) -> EnvironmentResult<ResolvedBinding> {
        validate_managed_binding(&binding)?;
        let digest =
            canonical_digest(&binding).map_err(|_| invalid("binding cannot be canonicalized"))?;
        let binding_ref = format!("binding:{}", digest.as_str());
        let record = DefinitionRecord::canonical(
            binding.root_id.as_str(),
            DefinitionKind::Binding,
            &binding_ref,
            &binding,
            now_ms(),
        )
        .map_err(definition_error)?;
        self.plane
            .definitions
            .install(&record)
            .await
            .map_err(definition_error)?;
        Ok(ResolvedBinding {
            binding_ref: binding_ref.parse().expect("binding ref"),
            capabilities: vec![
                EnvironmentCapability::Execution,
                EnvironmentCapability::SessionPreparation,
                EnvironmentCapability::SandboxFiles,
                EnvironmentCapability::SandboxControl,
            ],
            environment_id: ENVIRONMENT_ID.parse().expect("environment id"),
            limits: ResolvedBindingLimits {
                max_inline_input_bytes: NonZeroU64::new(
                    brain_protocol::MAX_MANAGED_TOOL_INPUT_BYTES as u64,
                )
                .unwrap(),
                max_inline_result_bytes: NonZeroU64::new(
                    brain_protocol::MAX_TOOL_TERMINAL_INLINE_BYTES as u64,
                )
                .unwrap(),
                max_wait_ms: 30_000,
            },
            recovery: RecoveryClass::Retained,
        })
    }

    async fn submit(&self, request: SubmitRequest) -> EnvironmentResult<SubmitReceipt> {
        validate_inline_input(&request.envelope.input)?;
        if operation_request_digest(&request.envelope) != request.envelope.request_digest {
            return Err(invalid("operation request_digest is not canonical"));
        }
        let route = self.route_for_submit(&request).await?;
        let installed = self.install_for_operation(&route, &request).await;
        self.settle_guest_result(&route, installed).await?;
        let reply = self.guest_submit_rpc(&route, request).await?;
        match reply {
            ResponseReply::Submit(receipt) => Ok(receipt),
            _ => Err(wrong_reply("submit")),
        }
    }

    async fn observe(&self, request: ObserveRequest) -> EnvironmentResult<OperationObservation> {
        let installed = self.resolve_operation_target(&request.operation).await?;
        match self
            .guest_rpc(&installed, RequestCall::Observe(request))
            .await?
        {
            ResponseReply::Observe(observation) => Ok(observation),
            _ => Err(wrong_reply("observe")),
        }
    }

    async fn cancel(&self, request: CancelRequest) -> EnvironmentResult<CancellationReceipt> {
        let installed = self.resolve_operation_target(&request.operation).await?;
        match self
            .guest_rpc(&installed, RequestCall::Cancel(request))
            .await?
        {
            ResponseReply::Cancel(receipt) => Ok(receipt),
            _ => Err(wrong_reply("cancel")),
        }
    }

    async fn acknowledge_terminal(
        &self,
        request: AcknowledgeTerminalRequest,
    ) -> EnvironmentResult<Acknowledgement> {
        let installed = self.resolve_operation_target(&request.operation).await?;
        match self
            .guest_rpc(&installed, RequestCall::AcknowledgeTerminal(request))
            .await?
        {
            ResponseReply::AcknowledgeTerminal(receipt) => Ok(receipt),
            _ => Err(wrong_reply("acknowledgement")),
        }
    }
}
