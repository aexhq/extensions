//! One session's arc on a Lambda MicroVM.
//!
//! Launch delivers the immutable physical target seal plus one generation-scoped guest-control
//! bearer in the run-hook payload. It contains no cloud or customer credential. Session secret
//! material is redeemed later over that authenticated control channel, after durable target
//! installation. The caller durably installs the MicroVM identity returned by
//! `RunMicrovm` before readiness checks, endpoint discovery, JWE minting, or any guest request.
//!
//! Environment deliberately has no independent keepalive loop and no unauthenticated guest resume
//! hook. The provider idle bound equals the target lifetime so a model or child-session wait cannot
//! suspend an otherwise live Tool.

use std::time::Duration;

use anyhow::{Context as _, bail};
use aws_sdk_lambdamicrovms::types::MicrovmState;
use environment_core::materialization::ControlToken;
use environment_wire::{CONTROL_AUTH_HEADER, RunPayload};

use crate::control::{
    AUTH_HEADER, Control, ControlError, ExactRunMicrovmRequest, Microvm, is_gone,
};

#[derive(Debug, thiserror::Error)]
pub enum LaunchFailure {
    #[error(transparent)]
    Run(#[from] ControlError),
}

/// What a launched, armed, reachable environment looks like to the brain.
#[derive(Clone)]
pub struct LaunchedEnvironment {
    pub microvm_id: String,
    /// `https://…`, no trailing slash.
    pub endpoint: String,
    /// The JWE for `X-aws-proxy-auth`.
    pub auth_token: String,
    /// The generation-scoped guest bearer. Formatting stays unavailable through ControlToken.
    pub control_token: ControlToken,
}

/// The only provider identity needed at the durable target-install boundary. Endpoint discovery
/// and JWE minting happen after that install, so a transient credential failure cannot turn a
/// successfully launched VM into an unlocatable eight-hour orphan.
#[derive(Clone)]
pub struct LaunchedTarget {
    pub microvm_id: String,
}

/// The run-hook payload the guest expects (`environment_wire::RunPayload`).
pub fn run_payload(payload: &RunPayload) -> anyhow::Result<String> {
    serde_json::to_string(payload).context("serializing the closed Environment run payload")
}

/// Replays a complete request that was durably sealed before the first provider dispatch. AWS
/// specifies `client_token` as the idempotency key; exact replay recovers the same provider target
/// before Environments installs its identity with the existing TARGET CAS.
pub async fn launch_exact(
    control: &Control,
    request: &ExactRunMicrovmRequest,
) -> Result<LaunchedTarget, LaunchFailure> {
    let vm = control.run_exact(request).await?;
    Ok(LaunchedTarget { microvm_id: vm.id })
}

/// `RunMicrovm` returns the endpoint as a bare host (`mvm-….on.aws`); make it a scheme-qualified
/// origin with no trailing slash, so `ws_url` and endpoint probes can build URLs off it directly.
#[must_use]
pub fn normalise_endpoint(endpoint: &str) -> String {
    let host = endpoint
        .trim()
        .trim_end_matches('/')
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    format!("https://{host}")
}

/// Polls until the VM reaches `want`. Terminal states short-circuit with an error.
pub async fn wait_for_state(
    control: &Control,
    id: &str,
    want: &MicrovmState,
    timeout: Duration,
) -> anyhow::Result<Microvm> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() >= deadline {
            bail!("microvm {id} did not reach {want:?} within {timeout:?}");
        }
        let vm = match tokio::time::timeout_at(deadline, control.get(id)).await {
            Err(_) => bail!("microvm {id} state read exceeded the {timeout:?} readiness bound"),
            Ok(result) => match result {
                Ok(vm) => vm,
                Err(ControlError::Retryable(e) | ControlError::Throttled(e)) => {
                    tracing::debug!(error = %e, "get_microvm retry");
                    tokio::time::sleep_until(
                        (tokio::time::Instant::now() + Duration::from_millis(500)).min(deadline),
                    )
                    .await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            },
        };
        if vm.state == *want {
            return Ok(vm);
        }
        if is_gone(&vm.state) {
            bail!("microvm {id} is {:?} while waiting for {want:?}", vm.state);
        }
        tokio::time::sleep_until(
            (tokio::time::Instant::now() + Duration::from_millis(500)).min(deadline),
        )
        .await;
    }
}

/// The WebSocket URL for a VM endpoint.
#[must_use]
pub fn ws_url(endpoint: &str) -> String {
    let host = endpoint
        .trim_end_matches('/')
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    format!("wss://{host}/")
}

pub type GuestSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Credential-redacted endpoint connection failure. AWS documents HTTP 502 from a MicroVM
/// endpoint as an application-process failure; callers deliberately distinguish it from a generic
/// network break so repeated 502s can fence the physical generation even while `GetMicrovm`
/// remains eventually-consistently `RUNNING`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GuestConnectError {
    #[error("MicroVM endpoint rejected the WebSocket handshake with HTTP {0}")]
    Http(u16),
    #[error("MicroVM endpoint connection failed")]
    Transport,
    #[error("MicroVM endpoint request is invalid")]
    InvalidRequest,
}

/// Connects the private Environments framing socket through the provider-authenticated endpoint.
pub async fn connect(environment: &LaunchedEnvironment) -> Result<GuestSocket, GuestConnectError> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    let mut request = ws_url(&environment.endpoint)
        .into_client_request()
        .map_err(|_| GuestConnectError::InvalidRequest)?;
    let auth = environment
        .auth_token
        .parse()
        .map_err(|_| GuestConnectError::InvalidRequest)?;
    request.headers_mut().insert(AUTH_HEADER, auth);
    let control_auth = environment
        .control_token
        .expose()
        .parse()
        .map_err(|_| GuestConnectError::InvalidRequest)?;
    request
        .headers_mut()
        .insert(CONTROL_AUTH_HEADER, control_auth);
    match tokio_tungstenite::connect_async(request).await {
        Ok((socket, _)) => Ok(socket),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            Err(GuestConnectError::Http(response.status().as_u16()))
        }
        Err(_) => Err(GuestConnectError::Transport),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_swaps_scheme_and_keeps_the_host() {
        assert_eq!(
            ws_url("https://abc123.lambda-microvm.eu-west-1.on.aws"),
            "wss://abc123.lambda-microvm.eu-west-1.on.aws/"
        );
        assert_eq!(ws_url("https://x/"), "wss://x/");
    }

    #[test]
    fn a_bare_host_endpoint_becomes_a_scheme_qualified_origin() {
        // RunMicrovm returns the endpoint with no scheme and no trailing slash.
        let host = "mvm-01234567-abcd-ef01-2345-6789abcdef01.lambda-microvm.eu-west-1.on.aws";
        assert_eq!(normalise_endpoint(host), format!("https://{host}"));
        assert_eq!(
            normalise_endpoint(&format!("https://{host}/")),
            format!("https://{host}")
        );
        // And the two URL builders compose off the normalised form.
        let n = normalise_endpoint(host);
        assert_eq!(format!("{n}/"), format!("https://{host}/"));
        assert_eq!(ws_url(&n), format!("wss://{host}/"));
    }

    #[test]
    fn the_run_payload_is_closed_and_contains_no_executor_credential() {
        let payload: RunPayload = serde_json::from_value(serde_json::json!({
            "contract_digest": "a".repeat(64),
            "generation": "generation-1",
            "expires_at_ms": 28800000,
            "root_id": "root-1",
            "owner_session_id": "session-1",
            "connector": "none",
            "resource_class": "microvm-1gb",
            "resources": {"max_output_bytes": 1024, "timeout_ms": 1000},
            "network": {"kind": "none"},
            "control_token": format!("control-{}", "a".repeat(64))
        }))
        .unwrap();
        let encoded = run_payload(&payload).unwrap();
        let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["generation"], "generation-1");
        assert!(value.get("token").is_none());
        assert!(value.get("credentials").is_none());
        assert!(value.get("canary_exit_after_operation_id").is_none());
    }
}
