//! Typed MicroVM lifecycle control over the AWS SDK, with every failure classified.
//!
//! Classification is the `environment_lost` contract: the brain needs to know whether a failed call
//! means *try again* ([`ControlError::Retryable`]), *the environment is gone* ([`ControlError::Gone`],
//! surfaced to the session as `environment_lost` and never replayed), or *the request itself is
//! wrong* ([`ControlError::Fatal`], a bug or a config error, loud and unretried).

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aws_sdk_lambdamicrovms::Client;
use aws_sdk_lambdamicrovms::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_lambdamicrovms::types::{IdlePolicy, MicrovmState, PortSpecification};
use environment_core::connector::ConnectorRef;
use serde::{Deserialize, Serialize};

use crate::{AGENT_PORT, MAX_DURATION_SECONDS, MAX_IDLE_SECONDS, TOKEN_TTL_SECONDS};

const CONTROL_CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// The JWE goes in this request header, verbatim, on every call through the VM endpoint.
pub const AUTH_HEADER: &str = "X-aws-proxy-auth";

/// The AWS-managed ingress connector that exposes the authenticated public endpoint.
pub const ALL_INGRESS: &str = "ALL_INGRESS";

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// A run was rejected before allocation because the plane exhausted capacity/quota. This is
    /// distinct from throttling and from an outcome-unknown transport failure: callers may safely
    /// release the materialization lease, but must not select a different connector or target.
    #[error("capacity exhausted ({scope}); retry after {retry_after_ms} ms: {message}")]
    Capacity {
        scope: String,
        retry_after_ms: u64,
        message: String,
    },
    /// The provider explicitly rejected admission before applying the operation. This is the one
    /// effect-side service response that may be retried without first reconciling target state.
    #[error("provider throttle: {0}")]
    Throttled(String),
    /// Throttle, transient server error, or a network failure on a read — try again.
    #[error("retryable: {0}")]
    Retryable(String),
    /// The VM no longer exists or is past recovery: surface `environment_lost` to the session.
    #[error("microvm gone: {0}")]
    Gone(String),
    /// A network failure during a state-changing call: the effect may or may not have taken.
    /// The caller must re-read (`get`) before deciding anything.
    #[error("outcome unknown: {0}")]
    Unknown(String),
    /// The request is wrong (validation, auth, quota misconfiguration). Not retryable.
    #[error("fatal: {0}")]
    Fatal(String),
}

/// One MicroVM as the control plane sees it.
#[derive(Debug, Clone)]
pub struct Microvm {
    pub id: String,
    pub state: MicrovmState,
    /// `https://…` — present once the VM has one.
    pub endpoint: Option<String>,
}

/// Every state-bearing `RunMicrovm` parameter, including the provider idempotency token. This type
/// deliberately has no `Debug`: `run_hook_payload` may contain a generation-scoped allowlist
/// capability. The production adapter stores its canonical serialization in the durable TARGET
/// reservation before dispatch and replays the exact value after an ambiguous response.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExactRunMicrovmRequest {
    pub image_identifier: String,
    pub image_version: String,
    pub ingress_network_connector: String,
    pub egress_network_connector: String,
    pub max_idle_duration_seconds: u64,
    pub suspended_duration_seconds: u64,
    pub auto_resume_enabled: bool,
    pub maximum_duration_seconds: u64,
    pub run_hook_payload: String,
    pub client_token: String,
}

#[derive(Clone)]
pub struct Control {
    client: Client,
    region: String,
    pacing: Arc<ControlPacing>,
}

/// Plane-process admission gates matching the observed us-east-1 operation limits. They are
/// intentionally operation-specific: a teardown storm cannot consume Run admission, and Run is
/// never retried here after an ambiguous provider outcome.
struct ControlPacing {
    run: TokenBucket,
    resume: TokenBucket,
    suspend: TokenBucket,
    terminate: TokenBucket,
}

/// Validated per-process Lambda MicroVM lifecycle ceilings. The defaults match the reduced
/// us-east-1 account observed during the MVP freeze, while the maxima match AWS's documented
/// public operation quotas. Raising a value is an explicit deployment change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlPacingConfig {
    pub run_rate_per_second: u32,
    pub run_burst: u32,
    pub resume_rate_per_second: u32,
    pub resume_burst: u32,
    pub suspend_rate_per_second: u32,
    pub suspend_burst: u32,
    pub terminate_rate_per_second: u32,
    pub terminate_burst: u32,
}

impl Default for ControlPacingConfig {
    fn default() -> Self {
        Self {
            run_rate_per_second: 1,
            run_burst: 1,
            resume_rate_per_second: 5,
            resume_burst: 5,
            suspend_rate_per_second: 2,
            suspend_burst: 2,
            terminate_rate_per_second: 10,
            terminate_burst: 10,
        }
    }
}

impl ControlPacingConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> anyhow::Result<Self> {
        let defaults = Self::default();
        let value = |name: &str,
                     default: u32,
                     maximum: u32,
                     lookup: &mut dyn FnMut(&str) -> Option<String>|
         -> anyhow::Result<u32> {
            let raw = lookup(name).unwrap_or_else(|| default.to_string());
            let parsed = raw.parse::<u32>().map_err(|_| {
                anyhow::anyhow!("{name} must be an integer from 1 through {maximum}")
            })?;
            anyhow::ensure!(
                (1..=maximum).contains(&parsed),
                "{name} must be from 1 through {maximum}"
            );
            Ok(parsed)
        };
        Ok(Self {
            run_rate_per_second: value(
                "ENVIRONMENT_PROVIDER_RUN_RATE_PER_SECOND",
                defaults.run_rate_per_second,
                5,
                &mut lookup,
            )?,
            run_burst: value(
                "ENVIRONMENT_PROVIDER_RUN_BURST",
                defaults.run_burst,
                5,
                &mut lookup,
            )?,
            resume_rate_per_second: value(
                "ENVIRONMENT_PROVIDER_RESUME_RATE_PER_SECOND",
                defaults.resume_rate_per_second,
                5,
                &mut lookup,
            )?,
            resume_burst: value(
                "ENVIRONMENT_PROVIDER_RESUME_BURST",
                defaults.resume_burst,
                5,
                &mut lookup,
            )?,
            suspend_rate_per_second: value(
                "ENVIRONMENT_PROVIDER_SUSPEND_RATE_PER_SECOND",
                defaults.suspend_rate_per_second,
                2,
                &mut lookup,
            )?,
            suspend_burst: value(
                "ENVIRONMENT_PROVIDER_SUSPEND_BURST",
                defaults.suspend_burst,
                2,
                &mut lookup,
            )?,
            terminate_rate_per_second: value(
                "ENVIRONMENT_PROVIDER_TERMINATE_RATE_PER_SECOND",
                defaults.terminate_rate_per_second,
                10,
                &mut lookup,
            )?,
            terminate_burst: value(
                "ENVIRONMENT_PROVIDER_TERMINATE_BURST",
                defaults.terminate_burst,
                10,
                &mut lookup,
            )?,
        })
    }
}

impl From<ControlPacingConfig> for ControlPacing {
    fn from(value: ControlPacingConfig) -> Self {
        Self {
            run: TokenBucket::new(value.run_rate_per_second, value.run_burst),
            resume: TokenBucket::new(value.resume_rate_per_second, value.resume_burst),
            suspend: TokenBucket::new(value.suspend_rate_per_second, value.suspend_burst),
            terminate: TokenBucket::new(value.terminate_rate_per_second, value.terminate_burst),
        }
    }
}

struct TokenBucket {
    interval: Duration,
    burst: u32,
    theoretical_arrival: Mutex<Option<Instant>>,
}

impl TokenBucket {
    fn new(rate_per_second: u32, burst: u32) -> Self {
        debug_assert!(rate_per_second > 0 && burst > 0);
        Self {
            interval: Duration::from_nanos(1_000_000_000 / u64::from(rate_per_second)),
            burst,
            theoretical_arrival: Mutex::new(None),
        }
    }

    fn reserve(&self, now: Instant) -> Duration {
        let mut theoretical_arrival = self.theoretical_arrival.lock().expect("token bucket lock");
        let prior = theoretical_arrival.unwrap_or(now);
        let tolerance = self.interval.saturating_mul(self.burst.saturating_sub(1));
        let earliest = prior.checked_sub(tolerance).unwrap_or(now);
        let admitted_at = earliest.max(now);
        *theoretical_arrival = Some(prior.max(admitted_at) + self.interval);
        admitted_at.saturating_duration_since(now)
    }

    async fn acquire(&self) {
        let delay = self.reserve(Instant::now());
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
}

impl Control {
    pub fn new(client: Client, region: impl Into<String>) -> Self {
        Self::with_pacing(client, region, ControlPacingConfig::default())
    }

    pub fn with_pacing(
        client: Client,
        region: impl Into<String>,
        pacing: ControlPacingConfig,
    ) -> Self {
        Self {
            client,
            region: region.into(),
            pacing: Arc::new(pacing.into()),
        }
    }

    /// Builds a paced control plane from an already-loaded shared `SdkConfig` (see
    /// [`crate::aws_config`]); pacing bounds still come from the environment.
    pub fn from_sdk_config(aws: &aws_config::SdkConfig, region: &str) -> anyhow::Result<Self> {
        Ok(Self::with_pacing(
            Client::new(aws),
            region,
            ControlPacingConfig::from_env()?,
        ))
    }

    async fn paced<T, F, Fut>(&self, gate: &TokenBucket, mut send: F) -> Result<T, ControlError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, ControlError>>,
    {
        const MAX_ATTEMPTS: u32 = 4;
        for attempt in 0..MAX_ATTEMPTS {
            gate.acquire().await;
            match send().await {
                Err(ControlError::Throttled(message)) if attempt + 1 < MAX_ATTEMPTS => {
                    let sample = rand::random::<u64>();
                    let delay = jittered_throttle_backoff(attempt, sample);
                    tracing::debug!(
                        attempt,
                        delay_ms = delay.as_millis(),
                        %message,
                        "provider throttled lifecycle operation"
                    );
                    tokio::time::sleep(delay).await;
                }
                result => return result,
            }
        }
        unreachable!("bounded provider retry loop returns")
    }

    fn connector_arn(&self, name: &str) -> String {
        if name.starts_with("arn:") {
            name.to_owned()
        } else {
            format!(
                "arn:aws:lambda:{}:aws:network-connector:aws-network-connector:{name}",
                self.region
            )
        }
    }

    /// Constructs the complete closed request used by ordinary Environments and image canaries. Callers
    /// that need crash recovery persist this value before calling [`Self::run_exact`].
    #[must_use]
    pub fn exact_run_request(
        &self,
        image_arn: &str,
        image_version: &str,
        run_hook_payload: &str,
        client_token: &str,
        egress_connector: &ConnectorRef,
    ) -> ExactRunMicrovmRequest {
        ExactRunMicrovmRequest {
            image_identifier: image_arn.to_owned(),
            image_version: image_version.to_owned(),
            ingress_network_connector: self.connector_arn(ALL_INGRESS),
            egress_network_connector: self.connector_arn(egress_connector.as_str()),
            max_idle_duration_seconds: MAX_IDLE_SECONDS,
            suspended_duration_seconds: MAX_DURATION_SECONDS,
            auto_resume_enabled: true,
            maximum_duration_seconds: MAX_DURATION_SECONDS,
            run_hook_payload: run_hook_payload.to_owned(),
            client_token: client_token.to_owned(),
        }
    }

    /// Validates a sealed request without comparing it to mutable deployment defaults. An old
    /// in-flight reservation must replay its exact image version and connector ARN after a Environment
    /// rollout, provided those immutable provider resources still exist.
    pub fn validate_exact_run_request(
        &self,
        request: &ExactRunMicrovmRequest,
    ) -> Result<(), ControlError> {
        if request.image_identifier.is_empty()
            || request.image_version.is_empty()
            || request.ingress_network_connector != self.connector_arn(ALL_INGRESS)
            || request.egress_network_connector.is_empty()
            || request.run_hook_payload.is_empty()
            || request.client_token.is_empty()
            || request.client_token.len() > 128
            || request.max_idle_duration_seconds == 0
            || request.max_idle_duration_seconds > request.maximum_duration_seconds
            || request.suspended_duration_seconds == 0
            || request.suspended_duration_seconds > MAX_DURATION_SECONDS
            || request.maximum_duration_seconds == 0
            || request.maximum_duration_seconds > MAX_DURATION_SECONDS
            || !request.auto_resume_enabled
        {
            return Err(ControlError::Fatal(
                "sealed RunMicrovm request is outside the supported boundary".into(),
            ));
        }
        Ok(())
    }

    /// Dispatches an already sealed request without deriving or defaulting any provider field.
    /// This is the production recovery path: an exact client-token replay is byte-identical even
    /// after the original process lost the successful provider response.
    pub async fn run_exact(
        &self,
        request: &ExactRunMicrovmRequest,
    ) -> Result<Microvm, ControlError> {
        self.validate_exact_run_request(request)?;
        let idle = IdlePolicy::builder()
            .max_idle_duration_seconds(request.max_idle_duration_seconds as i32)
            .suspended_duration_seconds(request.suspended_duration_seconds as i32)
            .auto_resume_enabled(request.auto_resume_enabled)
            .build()
            .map_err(|e| ControlError::Fatal(format!("idle policy: {e}")))?;
        // Deliberately no `.execution_role_arn(...)`: an execution role would make IAM
        // credentials retrievable from inside the guest via IMDSv2. The adversarial IMDS probe
        // confirmed the metadata service answers; only the *absence* of an attached role keeps it
        // empty. I8 —
        // the environment holds no cloud credential — depends on this staying unset.
        let out = self
            .paced(&self.pacing.run, || async {
                match tokio::time::timeout(
                    CONTROL_CALL_TIMEOUT,
                    self.client
                        .run_microvm()
                        .image_identifier(&request.image_identifier)
                        .image_version(&request.image_version)
                        .ingress_network_connectors(&request.ingress_network_connector)
                        .egress_network_connectors(&request.egress_network_connector)
                        .idle_policy(idle.clone())
                        .maximum_duration_in_seconds(request.maximum_duration_seconds as i32)
                        .run_hook_payload(&request.run_hook_payload)
                        .client_token(&request.client_token)
                        .send(),
                )
                .await
                {
                    Ok(result) => result.map_err(|error| effect_error(&error)),
                    Err(_) => Err(ControlError::Unknown(
                        "RunMicrovm timed out after dispatch".into(),
                    )),
                }
            })
            .await?;
        Ok(Microvm {
            id: out.microvm_id().to_owned(),
            state: out.state().clone(),
            endpoint: Some(out.endpoint().to_owned()).filter(|e| !e.is_empty()),
        })
    }

    pub async fn get(&self, id: &str) -> Result<Microvm, ControlError> {
        let out = tokio::time::timeout(
            CONTROL_CALL_TIMEOUT,
            self.client.get_microvm().microvm_identifier(id).send(),
        )
        .await
        .map_err(|_| ControlError::Retryable("GetMicrovm timed out".into()))?
        .map_err(|e| read_error(&e))?;
        Ok(Microvm {
            id: out.microvm_id().to_owned(),
            state: out.state().clone(),
            endpoint: Some(out.endpoint().to_owned()).filter(|e| !e.is_empty()),
        })
    }

    pub async fn suspend(&self, id: &str) -> Result<(), ControlError> {
        self.paced(&self.pacing.suspend, || async {
            match tokio::time::timeout(
                CONTROL_CALL_TIMEOUT,
                self.client.suspend_microvm().microvm_identifier(id).send(),
            )
            .await
            {
                Ok(result) => result.map(|_| ()).map_err(|e| effect_error(&e)),
                Err(_) => Err(ControlError::Unknown(
                    "SuspendMicrovm timed out after dispatch".into(),
                )),
            }
        })
        .await
    }

    pub async fn resume(&self, id: &str) -> Result<(), ControlError> {
        self.paced(&self.pacing.resume, || async {
            match tokio::time::timeout(
                CONTROL_CALL_TIMEOUT,
                self.client.resume_microvm().microvm_identifier(id).send(),
            )
            .await
            {
                Ok(result) => result.map(|_| ()).map_err(|e| effect_error(&e)),
                Err(_) => Err(ControlError::Unknown(
                    "ResumeMicrovm timed out after dispatch".into(),
                )),
            }
        })
        .await
    }

    pub async fn terminate(&self, id: &str) -> Result<(), ControlError> {
        self.paced(&self.pacing.terminate, || async {
            match tokio::time::timeout(
                CONTROL_CALL_TIMEOUT,
                self.client
                    .terminate_microvm()
                    .microvm_identifier(id)
                    .send(),
            )
            .await
            {
                Ok(result) => result.map(|_| ()).map_err(|e| effect_error(&e)),
                Err(_) => Err(ControlError::Unknown(
                    "TerminateMicrovm timed out after dispatch".into(),
                )),
            }
        })
        .await
    }

    /// Mints the endpoint JWE, scoped to the one agent port.
    pub async fn auth_token(&self, id: &str) -> Result<String, ControlError> {
        let out = tokio::time::timeout(
            CONTROL_CALL_TIMEOUT,
            self.client
                .create_microvm_auth_token()
                .microvm_identifier(id)
                .expiration_in_minutes((TOKEN_TTL_SECONDS / 60) as i32)
                .allowed_ports(PortSpecification::Port(i32::from(AGENT_PORT)))
                .send(),
        )
        .await
        .map_err(|_| ControlError::Retryable("auth token request timed out".into()))?
        // Minting another endpoint token is side-effect free at the execution boundary.
        .map_err(|e| read_error(&e))?;
        out.auth_token()
            .get(AUTH_HEADER)
            .filter(|t| !t.is_empty())
            .cloned()
            .ok_or_else(|| ControlError::Fatal("response carried no auth token".into()))
    }

    pub async fn list(&self) -> Result<Vec<MicrovmSummary>, ControlError> {
        let mut vms = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let out = self
                .client
                .list_microvms()
                .set_next_token(next)
                .send()
                .await
                .map_err(|e| read_error(&e))?;
            for item in out.items() {
                vms.push(MicrovmSummary {
                    id: item.microvm_id().to_owned(),
                    state: item.state().clone(),
                    image_arn: item.image_arn().to_owned(),
                    image_version: item.image_version().to_owned(),
                });
            }
            next = out.next_token().map(str::to_owned);
            if next.is_none() {
                return Ok(vms);
            }
        }
    }

    pub(crate) fn sdk(&self) -> &Client {
        &self.client
    }

    pub fn region(&self) -> &str {
        &self.region
    }
}

/// A listing row. `ListMicrovms` does not return endpoints, so the summary deliberately has no
/// endpoint field instead of an ambiguous `None`.
#[derive(Debug, Clone)]
pub struct MicrovmSummary {
    pub id: String,
    pub state: MicrovmState,
    pub image_arn: String,
    pub image_version: String,
}

/// Whether a VM state means the environment can never come back.
pub fn is_gone(state: &MicrovmState) -> bool {
    matches!(state, MicrovmState::Terminating | MicrovmState::Terminated)
}

/// Whether the provider has confirmed that this target no longer consumes materialized memory.
/// `Terminating` is irreversible for routing but may still occupy quota, so capacity accounting
/// must wait for `Terminated` (or an authoritative not-found response).
pub fn is_terminated(state: &MicrovmState) -> bool {
    matches!(state, MicrovmState::Terminated)
}

fn classify(code: Option<&str>, message: String, effect: bool) -> ControlError {
    match code {
        Some("ResourceNotFoundException") => ControlError::Gone(message),
        Some(code @ ("ServiceQuotaExceededException" | "ResourceLimitExceededException")) => {
            ControlError::Capacity {
                scope: code.to_owned(),
                retry_after_ms: 5_000,
                message,
            }
        }
        Some(code @ ("InsufficientCapacityException" | "InsufficientInstanceCapacity")) => {
            ControlError::Capacity {
                scope: code.to_owned(),
                retry_after_ms: 1_000,
                message,
            }
        }
        Some("ThrottlingException") | Some("TooManyRequestsException") => {
            ControlError::Throttled(message)
        }
        Some("InternalServerException") | Some("ServiceUnavailableException") if effect => {
            ControlError::Unknown(message)
        }
        Some("InternalServerException") | Some("ServiceUnavailableException") => {
            ControlError::Retryable(message)
        }
        Some("ConflictException") if effect => ControlError::Unknown(message),
        Some(_) => ControlError::Fatal(message),
        // No service error code: the transport failed.
        None if effect => ControlError::Unknown(message),
        None => ControlError::Retryable(message),
    }
}

fn jittered_throttle_backoff(attempt: u32, entropy: u64) -> Duration {
    // Full jitter avoids synchronized retries across task replicas. The deterministic entropy
    // parameter keeps the policy independently testable without sleeping or mocking the SDK.
    let cap_ms = 100u64.saturating_mul(1u64 << attempt.min(4)).min(2_000);
    Duration::from_millis(entropy % (cap_ms + 1))
}

fn effect_error<E: ProvideErrorMetadata, R>(e: &SdkError<E, R>) -> ControlError {
    sdk_error(e, true)
}

pub(crate) fn read_error<E: ProvideErrorMetadata, R>(e: &SdkError<E, R>) -> ControlError {
    sdk_error(e, false)
}

fn sdk_error<E: ProvideErrorMetadata, R>(e: &SdkError<E, R>, effect: bool) -> ControlError {
    match e {
        SdkError::ServiceError(s) => classify(
            s.err().code(),
            s.err().message().unwrap_or("service error").to_owned(),
            effect,
        ),
        SdkError::ConstructionFailure(_) => ControlError::Fatal("request construction".into()),
        other => classify(None, other.to_string(), effect),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_connector_names_expand_to_the_documented_arn() {
        let control = Control::new(
            Client::from_conf(aws_sdk_lambdamicrovms::Config::builder().build()),
            "eu-west-1",
        );
        assert_eq!(
            control.connector_arn(ALL_INGRESS),
            "arn:aws:lambda:eu-west-1:aws:network-connector:aws-network-connector:ALL_INGRESS"
        );
        let custom = "arn:aws:lambda:eu-west-1:123456789012:network-connector/custom";
        assert_eq!(control.connector_arn(custom), custom);
    }

    #[test]
    fn exact_run_request_seals_every_effect_parameter_and_client_token() {
        let control = Control::new(
            Client::from_conf(aws_sdk_lambdamicrovms::Config::builder().build()),
            "us-east-1",
        );
        let connector = ConnectorRef::parse(
            "arn:aws:lambda:us-east-1:111111111111:network-connector:aex-dev-none",
        )
        .unwrap();
        let request = control.exact_run_request(
            "arn:aws:lambda:us-east-1:111111111111:microvm-image:aex-dev",
            "7",
            "sealed-run-hook-payload",
            "stable-client-token",
            &connector,
        );
        let first = serde_json::to_vec(&request).unwrap();
        let replay: ExactRunMicrovmRequest = serde_json::from_slice(&first).unwrap();
        assert_eq!(serde_json::to_vec(&replay).unwrap(), first);
        assert_eq!(request.client_token, "stable-client-token");
        assert_eq!(request.max_idle_duration_seconds, MAX_IDLE_SECONDS);
        assert_eq!(request.max_idle_duration_seconds, MAX_DURATION_SECONDS);
        assert_eq!(request.maximum_duration_seconds, MAX_DURATION_SECONDS);
        assert_eq!(request.suspended_duration_seconds, MAX_DURATION_SECONDS);
        assert!(request.auto_resume_enabled);
        assert!(request.ingress_network_connector.ends_with(ALL_INGRESS));
        assert_eq!(request.egress_network_connector, connector.as_str());
        control.validate_exact_run_request(&request).unwrap();

        // Recovery validates the closed request itself, not a mutable deployment connector
        // catalog. An old-but-still-live connector ARN remains replayable after rollout.
        let mut prior_deployment = replay.clone();
        prior_deployment.egress_network_connector =
            "arn:aws:lambda:us-east-1:111111111111:network-connector:aex-dev-prior".into();
        control
            .validate_exact_run_request(&prior_deployment)
            .unwrap();
        prior_deployment.egress_network_connector.clear();
        assert!(
            control
                .validate_exact_run_request(&prior_deployment)
                .is_err()
        );

        let different = control.exact_run_request(
            &request.image_identifier,
            &request.image_version,
            &request.run_hook_payload,
            "different-client-token",
            &connector,
        );
        assert_ne!(
            serde_json::to_vec(&request).unwrap(),
            serde_json::to_vec(&different).unwrap()
        );
    }

    #[test]
    fn reads_retry_five_xx_but_effects_treat_it_as_an_ambiguous_outcome() {
        assert!(matches!(
            classify(Some("ResourceNotFoundException"), String::new(), true),
            ControlError::Gone(_)
        ));
        assert!(matches!(
            classify(Some("ThrottlingException"), String::new(), true),
            ControlError::Throttled(_)
        ));
        assert!(matches!(
            classify(Some("ServiceQuotaExceededException"), String::new(), true),
            ControlError::Capacity {
                retry_after_ms: 5_000,
                ..
            }
        ));
        assert!(matches!(
            classify(Some("SomethingNew"), String::new(), true),
            ControlError::Fatal(_)
        ));
        // A transport failure during an effect call must never be presumed to have failed.
        assert!(matches!(
            classify(None, String::new(), true),
            ControlError::Unknown(_)
        ));
        assert!(matches!(
            classify(None, String::new(), false),
            ControlError::Retryable(_)
        ));
        for code in ["InternalServerException", "ServiceUnavailableException"] {
            assert!(matches!(
                classify(Some(code), String::new(), true),
                ControlError::Unknown(_)
            ));
            assert!(matches!(
                classify(Some(code), String::new(), false),
                ControlError::Retryable(_)
            ));
        }
    }

    #[test]
    fn terminal_states_are_gone_and_the_rest_are_not() {
        assert!(is_gone(&MicrovmState::Terminated));
        assert!(is_gone(&MicrovmState::Terminating));
        for state in [
            MicrovmState::Pending,
            MicrovmState::Running,
            MicrovmState::Suspending,
            MicrovmState::Suspended,
        ] {
            assert!(!is_gone(&state), "{state:?}");
        }
        assert!(is_terminated(&MicrovmState::Terminated));
        assert!(!is_terminated(&MicrovmState::Terminating));
    }

    #[test]
    fn token_bucket_honours_burst_then_deterministically_paces() {
        let gate = TokenBucket::new(2, 2);
        let now = Instant::now();
        assert_eq!(gate.reserve(now), Duration::ZERO);
        assert_eq!(gate.reserve(now), Duration::ZERO);
        assert_eq!(gate.reserve(now), Duration::from_millis(500));
        assert_eq!(gate.reserve(now), Duration::from_secs(1));
    }

    #[test]
    fn pacing_configuration_defaults_to_the_observed_reduced_account_and_fails_closed() {
        let config = ControlPacingConfig::from_lookup(|_| None).unwrap();
        assert_eq!(config, ControlPacingConfig::default());
        assert_eq!(config.run_rate_per_second, 1);
        assert_eq!(config.run_burst, 1);
        assert_eq!(config.resume_rate_per_second, 5);
        assert_eq!(config.suspend_rate_per_second, 2);
        assert_eq!(config.terminate_rate_per_second, 10);

        let invalid = ControlPacingConfig::from_lookup(|name| {
            (name == "ENVIRONMENT_PROVIDER_RUN_RATE_PER_SECOND").then(|| "6".into())
        });
        assert!(invalid.is_err());
    }

    #[test]
    fn throttle_backoff_is_bounded_full_jitter() {
        assert_eq!(jittered_throttle_backoff(0, 0), Duration::ZERO);
        assert_eq!(
            jittered_throttle_backoff(0, 100),
            Duration::from_millis(100)
        );
        assert!(jittered_throttle_backoff(3, u64::MAX) <= Duration::from_millis(800));
        assert!(jittered_throttle_backoff(99, u64::MAX) <= Duration::from_millis(1_600));
    }
}
