//! Sealed provider launch: exact-replay dispatch inside the durable materialization lease.

use crate::*;

pub(crate) struct GenerationLauncher {
    pub(crate) plane: Arc<AwsEnvironmentPlane>,
    pub(crate) key: TargetKey,
    pub(crate) owner_session_id: String,
    pub(crate) resources: ResourceCeiling,
    pub(crate) network: NetworkCeiling,
    pub(crate) resource_class: String,
}

/// Full RunMicrovm parameter projection. It intentionally has no `Debug`: the nested run-hook
/// payload can contain the allowlist gateway bearer for this one private-network target generation.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SealedProviderLaunch {
    pub(crate) image_identity: String,
    pub(crate) dispatch_deadline_at_ms: u64,
    pub(crate) request: ExactRunMicrovmRequest,
}

impl GenerationLauncher {
    pub(crate) fn from_durable(
        plane: Arc<AwsEnvironmentPlane>,
        lease: &MaterializationLease,
    ) -> Result<Self, MaterializationError> {
        let sealed: SealedProviderLaunch = serde_json::from_str(lease.launch_request.expose())
            .map_err(|_| {
                MaterializationError::LaunchOutcomeUnknown(
                    "durable provider launch request is corrupt".into(),
                )
            })?;
        let payload: RunPayload =
            serde_json::from_str(&sealed.request.run_hook_payload).map_err(|_| {
                MaterializationError::LaunchOutcomeUnknown("durable run payload is corrupt".into())
            })?;
        Ok(Self {
            plane,
            key: lease.key.clone(),
            owner_session_id: payload.owner_session_id,
            resources: payload.resources,
            network: payload.network,
            resource_class: payload.resource_class,
        })
    }

    pub(crate) async fn seal_launch(
        &self,
        lease: &MaterializationLease,
    ) -> EnvironmentResult<DurableLaunchRequest> {
        let connector = connector_class(&self.network);
        let allowlist_proxy = if let NetworkCeiling::Allowlist(destinations) = &self.network {
            let issued_at_ms = now_ms();
            let capability = Capability {
                root_id: self.key.root_id.clone(),
                session_id: self.owner_session_id.clone(),
                sandbox_id: sandbox_identity(&self.key)?,
                generation: lease.generation.clone(),
                issued_at_ms,
                // The grant never outlives Brain's journaled physical target deadline. That
                // deadline is conservative (computed before KMS and provider dispatch), so it is
                // also no later than the provider's eight-hour VM wall.
                expires_at_ms: lease.target_expires_at_ms,
                policy_digest: canonical_digest(&self.network)
                    .expect("network is canonicalizable")
                    .to_string(),
                destinations: capability_destinations(destinations)?,
            };
            Some(AllowlistProxy {
                authority: self.plane.cfg.egress_gateway_authority.as_authority(),
                capability: self.plane.sign_capability(&capability).await?,
            })
        } else {
            None
        };
        let payload = RunPayload {
            contract_digest: ENVIRONMENT_CONTRACT_DIGEST.trim().into(),
            generation: lease.generation.clone(),
            expires_at_ms: lease.target_expires_at_ms,
            root_id: self.key.root_id.clone(),
            owner_session_id: self.owner_session_id.clone(),
            connector,
            resource_class: self.resource_class.clone(),
            resources: self.resources.clone(),
            network: self.network.clone(),
            control_token: ControlToken::new(format!(
                "control-{}",
                hex::encode(rand::random::<[u8; 32]>())
            ))
            .expect("random control token satisfies its exact grammar"),
            allowlist_proxy,
            canary_exit_after_operation_id: None,
        };
        let image_arn = self.plane.image_arn().await?;
        let image_version = self.plane.cfg.image_version.clone();
        let connector_ref = self.plane.cfg.connectors.resolve(connector).clone();
        let run_hook_payload = launch::run_payload(&payload)
            .map_err(|_| invalid("provider launch payload cannot be encoded"))?;
        let request = self.plane.control.exact_run_request(
            &image_arn,
            &image_version,
            &run_hook_payload,
            &lease.reservation_id,
            &connector_ref,
        );
        let sealed = SealedProviderLaunch {
            image_identity: lease.spec.image_identity.clone(),
            dispatch_deadline_at_ms: launch_dispatch_deadline(lease)
                .map_err(materialization_error)?,
            request,
        };
        let bytes = serde_jcs::to_vec(&sealed)
            .map_err(|_| invalid("provider launch request cannot be sealed"))?;
        let encoded = String::from_utf8(bytes)
            .map_err(|_| invalid("provider launch request is not UTF-8"))?;
        DurableLaunchRequest::new(encoded).map_err(|error| materialization_error(error.into()))
    }

    fn decode_launch(
        &self,
        lease: &MaterializationLease,
    ) -> Result<SealedProviderLaunch, LaunchError> {
        let sealed: SealedProviderLaunch = serde_json::from_str(lease.launch_request.expose())
            .map_err(|_| LaunchError::OutcomeUnknown("durable launch request is corrupt".into()))?;
        let payload: RunPayload = serde_json::from_str(&sealed.request.run_hook_payload)
            .map_err(|_| LaunchError::OutcomeUnknown("durable run payload is corrupt".into()))?;
        let resource_digest = canonical_digest(&payload.resources)
            .map_err(|_| LaunchError::OutcomeUnknown("durable resource seal is corrupt".into()))?;
        let network_digest = canonical_digest(&payload.network)
            .map_err(|_| LaunchError::OutcomeUnknown("durable network seal is corrupt".into()))?;
        let expected_dispatch_deadline = launch_dispatch_deadline(lease)
            .map_err(|error| LaunchError::OutcomeUnknown(error.to_string()))?;
        if payload.contract_digest != ENVIRONMENT_CONTRACT_DIGEST.trim()
            || payload.generation != lease.generation
            || payload.expires_at_ms != lease.target_expires_at_ms
            || payload.root_id != lease.key.root_id
            || payload.connector != lease.spec.connector
            || payload.resource_class != lease.spec.resource_class
            || sealed.image_identity != lease.spec.image_identity
            || sealed.dispatch_deadline_at_ms != expected_dispatch_deadline
            || resource_digest.as_str() != lease.spec.resource_policy_digest
            || network_digest.as_str() != lease.spec.network_policy_digest
            || payload.canary_exit_after_operation_id.is_some()
            || sealed.request.image_identifier.is_empty()
            || sealed.request.client_token != lease.reservation_id
        {
            return Err(LaunchError::OutcomeUnknown(
                "durable provider launch request conflicts with the target seal".into(),
            ));
        }
        if self
            .plane
            .control
            .validate_exact_run_request(&sealed.request)
            .is_err()
        {
            return Err(LaunchError::OutcomeUnknown(
                "durable provider request is outside the sealed exact RunMicrovm boundary".into(),
            ));
        }
        Ok(sealed)
    }
}

pub(crate) fn recovery_request(lease: &MaterializationLease, now_ms: u64) -> AcquireTarget {
    AcquireTarget {
        key: lease.key.clone(),
        spec: lease.spec.clone(),
        reservation_id: lease.reservation_id.clone(),
        generation: lease.generation.clone(),
        launch_request: lease.launch_request.clone(),
        attempt_id: random_identifier("purge-attempt"),
        attempt_duration_ms: TARGET_ATTEMPT_MS,
        generation_is_fenced: true,
        now_ms,
        lease_duration_ms: TARGET_LEASE_MS,
        target_lifetime_ms: TARGET_LIFETIME_MS,
        // Deletion is reconciliation of one exact existing row. It must never replace a gone
        // default target or create a fresh physical generation.
        replace_after_loss: false,
    }
}

pub(crate) fn recovery_launch_error(error_value: LaunchError) -> MaterializationError {
    match error_value {
        LaunchError::Capacity {
            scope,
            retry_after_ms,
            message,
        } => MaterializationError::Capacity {
            scope,
            retry_after_ms,
            message,
        },
        LaunchError::KnownNoTarget(message) => MaterializationError::LaunchOutcomeUnknown(format!(
            "exact launch recovery returned no target; reservation remains fenced: {message}"
        )),
        LaunchError::RetryableKnownNoTarget(message) => {
            MaterializationError::LaunchRetryable(message)
        }
        LaunchError::OutcomeUnknown(message) => MaterializationError::LaunchOutcomeUnknown(message),
    }
}

#[async_trait]
impl PhysicalTargetLauncher for GenerationLauncher {
    async fn launch(&self, lease: &MaterializationLease) -> Result<PhysicalTarget, LaunchError> {
        let sealed = self.decode_launch(lease)?;
        let control_token = serde_json::from_str::<RunPayload>(&sealed.request.run_hook_payload)
            .map_err(|_| LaunchError::OutcomeUnknown("durable run payload is corrupt".into()))?
            .control_token;
        admit_provider_dispatch(lease, sealed.dispatch_deadline_at_ms, now_ms())?;
        let environment = launch::launch_exact(&self.plane.control, &sealed.request)
            .await
            .map_err(|failure| match failure {
                LaunchFailure::Run(ControlError::Capacity {
                    scope,
                    retry_after_ms,
                    message,
                }) => LaunchError::Capacity {
                    scope,
                    retry_after_ms,
                    message,
                },
                LaunchFailure::Run(ControlError::Unknown(message)) => {
                    LaunchError::OutcomeUnknown(message)
                }
                LaunchFailure::Run(ControlError::Retryable(message))
                | LaunchFailure::Run(ControlError::Throttled(message)) => {
                    LaunchError::RetryableKnownNoTarget(message)
                }
                LaunchFailure::Run(ControlError::Fatal(message))
                | LaunchFailure::Run(ControlError::Gone(message)) => {
                    LaunchError::KnownNoTarget(message)
                }
            })?;
        PhysicalTarget::new(environment.microvm_id, lease.generation.clone(), control_token)
            .map_err(|error| LaunchError::OutcomeUnknown(error.to_string()))
    }

    async fn terminate_stale(&self, target: &PhysicalTarget) -> Result<(), String> {
        self.plane
            .control
            .terminate(&target.target_ref)
            .await
            .map_err(|error| error.to_string())
    }
}

pub(crate) fn launch_dispatch_deadline(
    lease: &MaterializationLease,
) -> Result<u64, MaterializationError> {
    let reserved_at_ms = lease
        .target_expires_at_ms
        .checked_sub(TARGET_LIFETIME_MS)
        .ok_or(MaterializationError::InvalidLease)?;
    let deadline = reserved_at_ms
        .checked_add(TARGET_DISPATCH_WINDOW_MS)
        .ok_or(MaterializationError::InvalidLease)?;
    if deadline >= lease.lease_expires_at_ms
        || lease.lease_expires_at_ms.saturating_sub(deadline) < TARGET_LIFETIME_MS
    {
        return Err(MaterializationError::InvalidLease);
    }
    Ok(deadline)
}

pub(crate) fn admit_provider_dispatch(
    lease: &MaterializationLease,
    dispatch_deadline_at_ms: u64,
    now_ms: u64,
) -> Result<(), LaunchError> {
    if now_ms <= dispatch_deadline_at_ms {
        return Ok(());
    }
    if lease.recovery_attempt {
        Err(LaunchError::OutcomeUnknown(
            "exact launch recovery window elapsed; possible target remains capacity-fenced".into(),
        ))
    } else {
        Err(LaunchError::KnownNoTarget(
            "provider dispatch deadline elapsed before the first RunMicrovm call".into(),
        ))
    }
}
