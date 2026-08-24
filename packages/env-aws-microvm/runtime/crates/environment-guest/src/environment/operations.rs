//! Operation lifecycle: admission, execution supervision, terminal retention, acknowledgement.

use super::*;

pub(crate) struct OperationMeta {
    pub(crate) operation: OperationRef,
    pub(crate) target: TargetReceipt,
    pub(crate) cancellation: CancellationToken,
    pub(crate) notify: Arc<Notify>,
    pub(crate) stdin: Option<Arc<InteractiveControl>>,
}

pub(crate) struct OperationBook {
    pub(crate) registry: OperationRegistry,
    pub(crate) metadata: HashMap<String, OperationMeta>,
}

impl Environment {
    pub async fn submit(
        self: &Arc<Self>,
        request: SubmitRequest,
    ) -> Result<SubmitReceipt, EnvironmentError> {
        validate_wait(request.wait_up_to_ms)?;
        if operation_request_digest(&request.envelope) != request.envelope.request_digest {
            return Err(invalid("operation request_digest is not canonical"));
        }
        self.fence_acknowledged_submission(
            request.envelope.operation_id.as_str(),
            request.envelope.request_digest.as_str(),
        )?;
        let execution = self.validate_operation(&request.envelope).await?;
        let operation = operation_ref(&request.envelope, &execution.target)?;
        let target = target_receipt(&execution.target)?;
        let cancellation = CancellationToken::new();
        let reservation = self
            .admit_operation(
                request.envelope.operation_id.as_str(),
                request.envelope.request_digest.as_str(),
                &operation,
                &target,
                &cancellation,
                None,
            )
            .await?;
        if reservation == Reservation::New {
            let execution_request = BundleExecution {
                bundle_path: execution.bundle_path,
                execute_digest: execution.execute_digest,
                node_path: execution.node_path,
                descriptor: execution.descriptor,
                envelope: request.envelope.clone(),
                workspace: self.cfg.workspace.clone(),
                runner: self.cfg.tool_runner.clone(),
                environment: execution.environment,
                proxy_environment: execution.target.proxy_environment,
                identity: execution.identity,
                boundary_library: self
                    .cfg
                    .sandboxing
                    .boundary_library()
                    .map(Path::to_path_buf),
                target_expires_at_ms: execution.target.expires_at_ms,
                cancellation,
            };
            self.spawn_execution(
                request.envelope.operation_id.to_string(),
                execute_bundle(execution_request),
            );
        }
        let observation = self
            .observe_inner(operation.clone(), request.wait_up_to_ms)
            .await?;
        Ok(SubmitReceipt {
            observation,
            operation,
            replayed: reservation == Reservation::Existing,
        })
    }

    pub async fn observe(
        &self,
        request: ObserveRequest,
    ) -> Result<OperationObservation, EnvironmentError> {
        validate_wait(request.wait_ms)?;
        self.observe_inner(request.operation, request.wait_ms).await
    }

    pub async fn cancel(
        &self,
        request: CancelRequest,
    ) -> Result<CancellationReceipt, EnvironmentError> {
        let (accepted, cancellation) = {
            let mut operations = self.operations.book.lock().await;
            validate_operation_ref(
                operations
                    .metadata
                    .get(request.operation.operation_id.as_str()),
                &request.operation,
            )?;
            let accepted = operations
                .registry
                .request_cancel(request.operation.operation_id.as_str())
                .map_err(operation_error)?;
            let cancellation = operations
                .metadata
                .get(request.operation.operation_id.as_str())
                .map(|meta| meta.cancellation.clone());
            (accepted, cancellation)
        };
        if accepted && let Some(cancellation) = cancellation {
            cancellation.cancel();
        }
        let observation = self.observe_inner(request.operation.clone(), 0).await?;
        Ok(CancellationReceipt {
            accepted,
            observation,
            operation: request.operation,
        })
    }

    pub async fn acknowledge_terminal(
        &self,
        request: AcknowledgeTerminalRequest,
    ) -> Result<Acknowledgement, EnvironmentError> {
        let acknowledgements = self.operations.acknowledgements.clone();
        let replay_operation = request.operation.clone();
        let replay_digest = request.terminal_digest.clone();
        let replayed = tokio::task::spawn_blocking(move || {
            acknowledgements.acknowledgement_exists(&replay_operation, &replay_digest)
        })
        .await
        .map_err(|_| unavailable("acknowledgement storage task failed"))?
        .map_err(ack_store_error)?;
        if replayed {
            self.release_acknowledged_terminal(&request.operation, &request.terminal_digest)
                .await?;
            return Ok(Acknowledgement { acknowledged: true });
        }

        {
            let operations = self.operations.book.lock().await;
            validate_operation_ref(
                operations
                    .metadata
                    .get(request.operation.operation_id.as_str()),
                &request.operation,
            )?;
            operations
                .registry
                .validate_terminal_ack(
                    request.operation.operation_id.as_str(),
                    request.terminal_digest.as_str(),
                )
                .map_err(operation_error)?;
        }

        let acknowledgements = self.operations.acknowledgements.clone();
        let operation = request.operation.clone();
        let terminal_digest = request.terminal_digest.clone();
        tokio::task::spawn_blocking(move || acknowledgements.retain(&operation, &terminal_digest))
            .await
            .map_err(|_| unavailable("acknowledgement storage task failed"))?
            .map_err(ack_store_error)?;

        // Concurrent exact acknowledgements may race after the durable tombstone. The first one
        // releases the payload; all others replay success from the same tombstone.
        self.release_acknowledged_terminal(&request.operation, &request.terminal_digest)
            .await?;
        Ok(Acknowledgement { acknowledged: true })
    }

    pub(crate) async fn release_acknowledged_terminal(
        &self,
        operation: &OperationRef,
        terminal_digest: &Digest,
    ) -> Result<(), EnvironmentError> {
        let mut operations = self.operations.book.lock().await;
        match operations
            .registry
            .acknowledge_terminal(operation.operation_id.as_str(), terminal_digest.as_str())
        {
            Ok(()) => {
                operations.metadata.remove(operation.operation_id.as_str());
            }
            // Exact replay after an earlier release or guest reconstruction needs no payload.
            Err(OperationError::Unknown) => {}
            Err(error) => return Err(operation_error(error)),
        }
        drop(operations);
        // Once Brain has durably committed and acknowledged the execution terminal, stdin
        // receipts for that execution no longer need generation-lifetime retention. Exact ACK
        // replay remains fenced by the durable payload-free acknowledgement log.
        self.stdin.book.lock().await.records.retain(|_, record| {
            !matches!(
                record,
                StdinRecord::Complete(receipt)
                    if receipt.observation.operation.operation_id == operation.operation_id
            )
        });
        Ok(())
    }

    pub async fn execute_sandbox(
        self: &Arc<Self>,
        request: SandboxExecutionRequest,
    ) -> Result<SubmitReceipt, EnvironmentError> {
        if sandbox_execution_request_digest(&request) != request.request_digest {
            return Err(invalid("sandbox execution request_digest is not canonical"));
        }
        self.fence_acknowledged_submission(
            request.execution_id.as_str(),
            request.request_digest.as_str(),
        )?;
        let target = self
            .fence(&request.target, request.expected_generation.as_str())
            .await?;
        validate_resource_subset(&request.resources, &target.resources)?;
        if !network_ceiling_is_subset(&request.network, &target.network) {
            return Err(environment_error(
                EnvironmentErrorCode::GenerationConflict,
                false,
                "sandbox execution network policy widens the immutable root target seal",
            ));
        }
        let cwd = request
            .input
            .cwd
            .as_ref()
            .map_or("/workspace", |cwd| cwd.as_str());
        if cwd.is_empty() {
            return Err(invalid(
                "sandbox execution cwd must be /workspace or a child path",
            ));
        }
        let files = self.workspace_files()?;
        let cwd = cwd.to_owned();
        let cwd = blocking_file(move || files.open_directory(&cwd)).await?;
        let operation = OperationRef {
            generation: target
                .generation
                .parse()
                .map_err(|_| invalid("generation is not a canonical operation locator"))?,
            operation_id: request.execution_id.clone(),
            receipt_ref: operation_receipt_ref(
                request.execution_id.as_str(),
                request.request_digest.as_str(),
                target.target_ref.as_str(),
                target.generation.as_str(),
            )?,
            request_digest: request.request_digest.clone(),
            target: request.target.clone(),
            target_ref: target
                .target_ref
                .parse()
                .map_err(|_| invalid("target_ref is not a canonical operation locator"))?,
        };
        let target_receipt = target_receipt(&target)?;
        let cancellation = CancellationToken::new();
        let control = request
            .input
            .interactive
            .then(|| Arc::new(InteractiveControl::default()));
        let reservation = self
            .admit_operation(
                request.execution_id.as_str(),
                request.request_digest.as_str(),
                &operation,
                &target_receipt,
                &cancellation,
                control.clone(),
            )
            .await?;
        if reservation == Reservation::New {
            let execution_request = ShellExecution {
                command: request.input.command.to_string(),
                cwd,
                workspace: self.cfg.workspace.clone(),
                timeout_ms: request.resources.timeout_ms.get(),
                max_output_bytes: request.resources.max_output_bytes.get(),
                interactive: request.input.interactive,
                proxy_environment: target.proxy_environment,
                identity: self.cfg.sandboxing.identity(),
                boundary_library: self
                    .cfg
                    .sandboxing
                    .boundary_library()
                    .map(Path::to_path_buf),
                target_expires_at_ms: target.expires_at_ms,
                cancellation,
                control,
            };
            self.spawn_execution(
                request.execution_id.to_string(),
                execute_shell(execution_request),
            );
        }
        let observation = self.observe_inner(operation.clone(), 0).await?;
        Ok(SubmitReceipt {
            observation,
            operation,
            replayed: reservation == Reservation::Existing,
        })
    }

    /// Reserves the operation id under the terminal-envelope byte reservation, records its
    /// metadata, and marks it running — or, on an exact replay, validates the existing record.
    async fn admit_operation(
        &self,
        operation_id: &str,
        request_digest: &str,
        operation: &OperationRef,
        target: &TargetReceipt,
        cancellation: &CancellationToken,
        stdin: Option<Arc<InteractiveControl>>,
    ) -> Result<Reservation, EnvironmentError> {
        let mut operations = self.operations.book.lock().await;
        let reservation = operations
            .registry
            .reserve(operation_id, request_digest, TERMINAL_ENVELOPE_BYTES)
            .map_err(operation_error)?;
        if reservation == Reservation::New {
            operations.metadata.insert(
                operation_id.to_string(),
                OperationMeta {
                    operation: operation.clone(),
                    target: target.clone(),
                    cancellation: cancellation.clone(),
                    notify: Arc::new(Notify::new()),
                    stdin,
                },
            );
            operations
                .registry
                .mark_running(operation_id)
                .map_err(operation_error)?;
        } else {
            validate_operation_ref(operations.metadata.get(operation_id), operation)?;
        }
        Ok(reservation)
    }

    pub(crate) async fn validate_operation(
        &self,
        envelope: &OperationEnvelope,
    ) -> Result<ValidatedExecution, EnvironmentError> {
        let target = self.require_target().await?;
        if envelope.root_id.as_str() != target.root_id
            || envelope
                .generation
                .as_ref()
                .is_some_and(|generation| generation.as_str() != target.generation)
            || envelope
                .target_ref
                .as_ref()
                .is_some_and(|target_ref| target_ref.as_str() != target.target_ref)
        {
            return Err(generation_conflict());
        }
        validate_resource_subset(&envelope.resources, &target.resources)?;
        if !network_ceiling_is_subset(&envelope.network, &target.network) {
            return Err(environment_error(
                EnvironmentErrorCode::GenerationConflict,
                false,
                "operation network policy widens the immutable root target seal",
            ));
        }
        let bindings = self.artifacts.bindings.read().await;
        let binding = bindings.get(envelope.binding_ref.as_str()).ok_or_else(|| {
            environment_error(
                EnvironmentErrorCode::BindingConflict,
                false,
                "binding_ref is not installed",
            )
        })?;
        if binding.seal.root_id != envelope.root_id
            || binding.seal.session_id != envelope.session_id
            || binding.seal.capability != envelope.capability
        {
            return Err(environment_error(
                EnvironmentErrorCode::BindingConflict,
                false,
                "operation does not match the immutable binding seal",
            ));
        }
        let descriptor = binding.seal.bundle.clone().ok_or_else(|| {
            environment_error(
                EnvironmentErrorCode::CapabilityUnavailable,
                false,
                "managed binding has no Tool bundle",
            )
        })?;
        let secrets = self.artifacts.secrets.read().await;
        let values = secrets.get(envelope.session_id.as_str());
        let mut environment = HashMap::new();
        for name in &descriptor.required_env {
            let value = values
                .and_then(|values| values.values.get(name.as_str()))
                .ok_or_else(|| unavailable("required Tool environment has not been delivered"))?;
            environment.insert(name.to_string(), value.clone());
        }
        Ok(ValidatedExecution {
            bundle_path: binding.bundle_path.clone(),
            execute_digest: binding.execute_digest.clone(),
            node_path: binding.node_path.clone(),
            descriptor,
            environment,
            identity: binding.identity,
            target,
        })
    }

    pub(crate) fn fence_acknowledged_submission(
        &self,
        operation_id: &str,
        request_digest: &str,
    ) -> Result<(), EnvironmentError> {
        match self
            .operations
            .acknowledgements
            .fence_submission(operation_id, request_digest)
            .map_err(ack_store_error)?
        {
            SubmissionFence::Clear => Ok(()),
            SubmissionFence::Acknowledged => Err(environment_error(
                EnvironmentErrorCode::OperationUnknown,
                false,
                "operation terminal was already committed and released",
            )),
        }
    }

    /// Runs one admitted execution through to its terminal record. The body is supervised: a
    /// panic in the executor or in `finish` must never leave the operation `Running` forever, so
    /// the supervisor records an interrupted terminal instead of dying silently.
    pub(crate) fn spawn_execution(
        self: &Arc<Self>,
        operation_id: String,
        run: impl Future<Output = crate::process::ExecutionResult> + Send + 'static,
    ) {
        let environment = self.clone();
        tokio::spawn(async move {
            let supervised = tokio::spawn({
                let environment = environment.clone();
                let operation_id = operation_id.clone();
                async move {
                    let _slot = match environment.operations.slots.clone().acquire_owned().await {
                        Ok(slot) => slot,
                        Err(_) => {
                            tracing::error!(
                                operation_id,
                                "operation slots closed before execution started"
                            );
                            return;
                        }
                    };
                    let result = run.await;
                    environment.finish(&operation_id, result).await;
                }
            });
            if let Err(join_error) = supervised.await {
                tracing::error!(
                    operation_id,
                    %join_error,
                    "execution task died before recording a terminal; recording interrupted"
                );
                environment.finish(
                    &operation_id,
                    crate::process::ExecutionResult {
                        outcome: TerminalOutcome::Interrupted,
                        inline: serde_json::json!({
                            "error": "execution was interrupted by an internal fault before a terminal result was recorded"
                        }),
                        is_error: true,
                        exit_code: None,
                        duration_ms: 0,
                    },
                )
                .await;
            }
        });
    }

    pub(crate) async fn finish(
        &self,
        operation_id: &str,
        mut result: crate::process::ExecutionResult,
    ) {
        // The child boundary already enforces the operation's narrower output ceiling. Keep this
        // final check at the receipt boundary as defense in depth: a future executor must never
        // retain a success that Brain cannot journal after the effect has happened.
        if !terminal_inline_fits(&result.inline) {
            result.inline = serde_json::json!({
                "error": "execution may have completed, but its inline result exceeded the Brain terminal limit; store large data in session storage or the sandbox and return a key/path"
            });
            result.is_error = true;
            result.outcome = TerminalOutcome::Failed;
        }
        let mut terminal = TerminalResult {
            duration_ms: Some(result.duration_ms),
            exit_code: result.exit_code,
            inline: Some(result.inline),
            is_error: result.is_error,
            object: None,
            outcome: result.outcome,
            terminal_digest: "0".repeat(64).parse().expect("digest placeholder"),
        };
        terminal.terminal_digest = terminal_result_digest(&terminal);
        let mut operations = self.operations.book.lock().await;
        let Some(meta) = operations.metadata.get(operation_id) else {
            // The child already ran: discarding its effect must at least be visible.
            tracing::warn!(
                operation_id,
                "terminal result for an unknown operation was discarded"
            );
            return;
        };
        let operation = meta.operation.clone();
        let target = meta.target.clone();
        let notify = meta.notify.clone();
        let observation = OperationObservation {
            next_cursor: "1".parse().expect("cursor"),
            operation: operation.clone(),
            output: Vec::new(),
            state: ContractOperationState::Terminal,
            target: Some(target.clone()),
            terminal: Some(terminal.clone()),
        };
        let completed = serde_json::to_vec(&observation).is_ok_and(|payload| {
            operations
                .registry
                .complete(operation_id, terminal.terminal_digest.as_str(), payload)
                .is_ok()
        });
        if completed {
            notify.notify_waiters();
            return;
        }

        // This should be unreachable after admission reserves output plus worst-case encoded
        // diagnostics. Still fail terminally instead of retaining a fictitious `running` state if
        // a future contract shape grows beyond that calculation.
        let mut fallback = TerminalResult {
            duration_ms: Some(result.duration_ms),
            exit_code: result.exit_code,
            inline: Some(serde_json::json!({
                "error": "terminal result could not be retained within its reserved capacity"
            })),
            is_error: true,
            object: None,
            outcome: TerminalOutcome::Interrupted,
            terminal_digest: "0".repeat(64).parse().expect("digest placeholder"),
        };
        fallback.terminal_digest = terminal_result_digest(&fallback);
        let fallback_observation = OperationObservation {
            next_cursor: "1".parse().expect("cursor"),
            operation,
            output: Vec::new(),
            state: ContractOperationState::Terminal,
            target: Some(target),
            terminal: Some(fallback.clone()),
        };
        if let Ok(payload) = serde_json::to_vec(&fallback_observation)
            && operations
                .registry
                .complete(operation_id, fallback.terminal_digest.as_str(), payload)
                .is_ok()
        {
            notify.notify_waiters();
        }
    }

    pub(crate) async fn observe_inner(
        &self,
        operation: OperationRef,
        wait_ms: u64,
    ) -> Result<OperationObservation, EnvironmentError> {
        let notify = {
            let operations = self.operations.book.lock().await;
            validate_operation_ref(
                operations.metadata.get(operation.operation_id.as_str()),
                &operation,
            )?;
            operations
                .registry
                .observe(operation.operation_id.as_str())
                .ok_or_else(|| operation_error(OperationError::Unknown))?;
            operations
                .metadata
                .get(operation.operation_id.as_str())
                .map(|meta| meta.notify.clone())
                .ok_or_else(|| operation_error(OperationError::Unknown))?
        };
        if wait_ms > 0 {
            // Enable the owned notification before the second state check. `notify_waiters`
            // does not retain a permit for a future waiter, so checking once and then creating a
            // waiter has a race that can add the entire 30-second observe window after terminal.
            let notified = notify.notified_owned();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let terminal = {
                let operations = self.operations.book.lock().await;
                matches!(
                    operations
                        .registry
                        .observe(operation.operation_id.as_str())
                        .map(|record| &record.state),
                    Some(OperationState::Terminal { .. })
                )
            };
            if !terminal {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(wait_ms.min(MAX_WAIT_MS)),
                    notified,
                )
                .await;
            }
        }
        let operations = self.operations.book.lock().await;
        let record = operations
            .registry
            .observe(operation.operation_id.as_str())
            .ok_or_else(|| operation_error(OperationError::Unknown))?;
        let meta = operations
            .metadata
            .get(operation.operation_id.as_str())
            .ok_or_else(|| operation_error(OperationError::Unknown))?;
        match &record.state {
            OperationState::Terminal { payload, .. } => serde_json::from_slice(payload)
                .map_err(|_| unavailable("retained terminal observation is unavailable")),
            OperationState::Accepted | OperationState::Running => Ok(OperationObservation {
                next_cursor: "0".parse().expect("cursor"),
                operation,
                output: Vec::new(),
                state: match record.state {
                    OperationState::Accepted => ContractOperationState::Accepted,
                    OperationState::Running => ContractOperationState::Running,
                    OperationState::Terminal { .. } => unreachable!(),
                },
                target: Some(meta.target.clone()),
                terminal: None,
            }),
        }
    }
}

pub(crate) struct ValidatedExecution {
    pub(crate) bundle_path: PathBuf,
    pub(crate) execute_digest: String,
    pub(crate) node_path: PathBuf,
    pub(crate) descriptor: BundleDescriptor,
    pub(crate) environment: HashMap<String, String>,
    pub(crate) identity: Option<ToolIdentity>,
    pub(crate) target: TargetSnapshot,
}
