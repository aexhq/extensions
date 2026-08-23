//! Armed-target state: arm, fence, snapshots, and canary exit.

use super::*;

/// No `Debug`: an allowlist target contains a bearer capability.
pub(crate) struct ArmedTarget {
    pub(crate) target_ref: String,
    pub(crate) generation: String,
    pub(crate) expires_at_ms: u64,
    pub(crate) root_id: String,
    pub(crate) owner_session_id: String,
    pub(crate) connector: ConnectorClass,
    pub(crate) resource_class: String,
    pub(crate) resources: ResourceCeiling,
    pub(crate) network: NetworkCeiling,
    pub(crate) control_token: environment_core::materialization::ControlToken,
    pub(crate) proxy_environment: HashMap<String, String>,
    pub(crate) canary_exit_after_operation_id: Option<String>,
}

impl Environment {
    pub async fn armed(&self) -> bool {
        self.target.armed.read().await.is_some()
    }

    /// Checks the generation-scoped bearer without exposing either value through formatting.
    /// Hashing both sides gives the final comparison a fixed width even for hostile headers.
    pub async fn control_authorized(&self, candidate: Option<&str>) -> bool {
        let Some(candidate) = candidate else {
            return false;
        };
        let target = self.target.armed.read().await;
        let Some(expected) = target.as_ref().map(|target| target.control_token.expose()) else {
            return false;
        };
        let expected_hash = Sha256::digest(expected.as_bytes());
        let candidate_hash = Sha256::digest(candidate.as_bytes());
        let difference = expected_hash
            .iter()
            .zip(candidate_hash.iter())
            .fold(0u8, |difference, (expected, candidate)| {
                difference | (expected ^ candidate)
            });
        difference == 0 && candidate.len() == expected.len()
    }

    /// Arms an unconfigured generation exactly once. An exact provider retry is harmless; a
    /// different root/generation/network/resource seal is a permanent conflict.
    pub async fn arm(&self, target_ref: String, payload: RunPayload) -> Result<bool, EnvironmentError> {
        if payload.contract_digest != ENVIRONMENT_CONTRACT_DIGEST.trim() {
            return Err(invalid("Environment contract digest does not match the image"));
        }
        let now = wall_ms();
        if payload.expires_at_ms <= now
            || payload.expires_at_ms > now.saturating_add(MAX_TARGET_LIFETIME_MS)
        {
            return Err(invalid(
                "physical target expiry is outside the supported lifetime",
            ));
        }
        validate_connector(
            payload.connector,
            &payload.network,
            payload.allowlist_proxy.is_some(),
        )?;
        if payload
            .canary_exit_after_operation_id
            .as_ref()
            .is_some_and(|id| {
                !id.starts_with("image-canary-")
                    || id.parse::<brain_protocol::environment::Identifier>().is_err()
            })
        {
            return Err(invalid("image canary operation id is invalid"));
        }
        let proxy_environment = match payload.allowlist_proxy {
            Some(proxy) => {
                let proxy_url = format!("http://aex:{}@{}", proxy.capability, proxy.authority);
                HashMap::from([
                    ("HTTPS_PROXY".into(), proxy_url.clone()),
                    ("https_proxy".into(), proxy_url),
                ])
            }
            None => HashMap::new(),
        };
        let candidate = ArmedTarget {
            target_ref,
            generation: payload.generation,
            expires_at_ms: payload.expires_at_ms,
            root_id: payload.root_id,
            owner_session_id: payload.owner_session_id,
            connector: payload.connector,
            resource_class: payload.resource_class,
            resources: payload.resources,
            network: payload.network,
            control_token: payload.control_token,
            proxy_environment,
            canary_exit_after_operation_id: payload.canary_exit_after_operation_id,
        };
        let mut target = self.target.armed.write().await;
        if let Some(existing) = target.as_ref() {
            let exact = existing.target_ref == candidate.target_ref
                && existing.generation == candidate.generation
                && existing.expires_at_ms == candidate.expires_at_ms
                && existing.root_id == candidate.root_id
                && existing.owner_session_id == candidate.owner_session_id
                && existing.connector == candidate.connector
                && existing.resource_class == candidate.resource_class
                && canonical_equal(&existing.resources, &candidate.resources)?
                && canonical_equal(&existing.network, &candidate.network)?
                && existing.control_token == candidate.control_token
                && existing.canary_exit_after_operation_id
                    == candidate.canary_exit_after_operation_id;
            return if exact {
                Ok(true)
            } else {
                Err(environment_error(
                    EnvironmentErrorCode::GenerationConflict,
                    false,
                    "physical generation is already armed with a different immutable seal",
                ))
            };
        }
        *target = Some(candidate);
        Ok(false)
    }

    pub async fn runtime_status(&self) -> Option<TargetRuntimeStatus> {
        self.target
            .armed
            .read()
            .await
            .as_ref()
            .map(|target| TargetRuntimeStatus {
                target_ref: target.target_ref.clone(),
                generation: target.generation.clone(),
                root_id: target.root_id.clone(),
                owner_session_id: target.owner_session_id.clone(),
                connector: target.connector,
                resource_class: target.resource_class.clone(),
                armed: true,
            })
    }

    pub async fn should_exit_after_canary_receipt(&self, operation_id: &str) -> bool {
        self.target
            .armed
            .read()
            .await
            .as_ref()
            .and_then(|target| target.canary_exit_after_operation_id.as_deref())
            == Some(operation_id)
    }

    pub(crate) async fn require_target(&self) -> Result<TargetSnapshot, EnvironmentError> {
        let target = self
            .target
            .armed
            .read()
            .await
            .as_ref()
            .map(TargetSnapshot::from)
            .ok_or_else(|| {
                environment_error(
                    EnvironmentErrorCode::SandboxNotMaterialized,
                    false,
                    "physical generation has not been armed",
                )
            })?;
        if wall_ms() >= target.expires_at_ms {
            return Err(environment_error(
                EnvironmentErrorCode::SandboxGone,
                false,
                "physical sandbox generation reached its hard deadline",
            ));
        }
        Ok(target)
    }

    pub(crate) async fn fence(
        &self,
        target: &brain_protocol::environment::SandboxTarget,
        generation: &str,
    ) -> Result<TargetSnapshot, EnvironmentError> {
        let physical = self.require_target().await?;
        if target.root_id.as_str() != physical.root_id || generation != physical.generation {
            return Err(generation_conflict());
        }
        Ok(physical)
    }
}
#[derive(Clone)]
pub(crate) struct TargetSnapshot {
    pub(crate) target_ref: String,
    pub(crate) generation: String,
    pub(crate) expires_at_ms: u64,
    pub(crate) root_id: String,
    pub(crate) resources: ResourceCeiling,
    pub(crate) network: NetworkCeiling,
    pub(crate) proxy_environment: HashMap<String, String>,
}

impl From<&ArmedTarget> for TargetSnapshot {
    fn from(target: &ArmedTarget) -> Self {
        Self {
            target_ref: target.target_ref.clone(),
            generation: target.generation.clone(),
            expires_at_ms: target.expires_at_ms,
            root_id: target.root_id.clone(),
            resources: target.resources.clone(),
            network: target.network.clone(),
            proxy_environment: target.proxy_environment.clone(),
        }
    }
}
