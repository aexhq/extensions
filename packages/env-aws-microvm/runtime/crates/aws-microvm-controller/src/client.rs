//! Authenticated, reconnecting client for one installed guest generation.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::temporary_from;
use aws_sdk_lambdamicrovms::types::MicrovmState;
use base64::Engine as _;
use brain_protocol::contract::ENVIRONMENT_CONTRACT_DIGEST;
use brain_protocol::environment::{EnvironmentError, EnvironmentErrorCode};
use futures_util::{SinkExt as _, StreamExt as _};
use environment_core::materialization::InstalledTarget;
use environment_lambda::control::{AUTH_HEADER, Control, ControlError, is_terminated};
use environment_lambda::launch::{self, LaunchedEnvironment};
use environment_wire::{
    CONTROL_AUTH_HEADER, FILE_ENTRY_HEADER, MAX_BUNDLE_INSTALL_BYTES, MAX_INSTALL_BODY_BYTES,
    MAX_INSTALL_METADATA_BYTES, MAX_WIRE_FRAME_BYTES, OBJECT_METADATA_HEADER, RequestCall,
    RequestFrame, ResponseFrame, ResponseReply,
};
use tokio::io::AsyncReadExt as _;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::io::ReaderStream;

const AUTH_REFRESH_AFTER: Duration = Duration::from_secs(50 * 60);
// The provider returns a target identity while RunMicrovm is still PENDING. External traffic is
// admitted only after the /run hook succeeds and the target reaches RUNNING; the image gives that
// hook 60 seconds, so the first guest request owns a small additional scheduling margin.
const INITIAL_TARGET_READY_TIMEOUT: Duration = Duration::from_secs(65);
const RPC_TIMEOUT: Duration = Duration::from_secs(45);

enum RpcAttemptError {
    EndpointApplicationUnavailable,
    Environment(EnvironmentError),
}

impl From<EnvironmentError> for RpcAttemptError {
    fn from(value: EnvironmentError) -> Self {
        Self::Environment(value)
    }
}

struct CachedEndpoint {
    environment: LaunchedEnvironment,
    minted_at: Instant,
}

/// Clone shares the provider connection pools and endpoint/JWE cache.
#[derive(Clone)]
pub struct GuestClient {
    control: Control,
    http: reqwest::Client,
    endpoints: Arc<RwLock<HashMap<String, CachedEndpoint>>>,
    next_request: Arc<AtomicU64>,
}

impl GuestClient {
    pub fn new(control: Control, http: reqwest::Client) -> Self {
        Self {
            control,
            http,
            endpoints: Arc::new(RwLock::new(HashMap::new())),
            next_request: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Shared bounded HTTP pool used only for one-purpose bundle/object authorities.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub async fn remember(&self, environment: LaunchedEnvironment) {
        self.endpoints.write().await.insert(
            environment.microvm_id.clone(),
            CachedEndpoint {
                environment,
                minted_at: Instant::now(),
            },
        );
    }

    pub async fn forget(&self, target_ref: &str) {
        self.endpoints.write().await.remove(target_ref);
    }

    async fn endpoint(&self, target: &InstalledTarget) -> Result<LaunchedEnvironment, EnvironmentError> {
        let target_ref = target.target_ref.as_str();
        if let Some(endpoint) = self.endpoints.read().await.get(target_ref)
            && endpoint.minted_at.elapsed() < AUTH_REFRESH_AFTER
        {
            return Ok(endpoint.environment.clone());
        }
        let mut vm = self.control.get(target_ref).await.map_err(control_error)?;
        if vm.state == MicrovmState::Pending {
            vm = launch::wait_for_state(
                &self.control,
                target_ref,
                &MicrovmState::Running,
                INITIAL_TARGET_READY_TIMEOUT,
            )
            .await
            .map_err(|error| {
                temporary_from("physical sandbox endpoint did not become ready", error)
            })?;
        }
        if is_terminated(&vm.state) {
            return Err(error(
                EnvironmentErrorCode::SandboxGone,
                false,
                "physical sandbox generation is gone",
            ));
        }
        if vm.state == MicrovmState::Terminating {
            return Err(temporary("physical sandbox generation is terminating"));
        }
        let endpoint = vm.endpoint.ok_or_else(|| {
            error(
                EnvironmentErrorCode::TemporarilyUnavailable,
                true,
                "physical sandbox endpoint is not ready",
            )
        })?;
        let auth_token = self
            .control
            .auth_token(target_ref)
            .await
            .map_err(control_error)?;
        let environment = LaunchedEnvironment {
            microvm_id: target_ref.into(),
            endpoint: launch::normalise_endpoint(&endpoint),
            auth_token,
            control_token: target.control_token.clone(),
        };
        self.remember(environment.clone()).await;
        Ok(environment)
    }

    pub async fn rpc(
        &self,
        target: &InstalledTarget,
        call: RequestCall,
    ) -> Result<ResponseReply, EnvironmentError> {
        let frame = RequestFrame {
            request_id: format!(
                "request-{}",
                self.next_request.fetch_add(1, Ordering::Relaxed)
            ),
            contract_digest: ENVIRONMENT_CONTRACT_DIGEST.trim().into(),
            call,
        };
        let encoded = serde_json::to_string(&frame).map_err(|_| {
            error(
                EnvironmentErrorCode::InvalidRequest,
                false,
                "request cannot be encoded",
            )
        })?;
        if encoded.len() > MAX_WIRE_FRAME_BYTES {
            return Err(error(
                EnvironmentErrorCode::ResourceExhausted,
                false,
                "request exceeds the Environment wire bound",
            ));
        }
        // Exact operation/digest requests are safe to replay after a transport break. Refreshing
        // the provider JWE never changes the target or connector.
        let mut saw_application_unavailable = false;
        for attempt in 0..2 {
            let endpoint = self.endpoint(target).await?;
            let result: Result<ResponseReply, RpcAttemptError> =
                match tokio::time::timeout(RPC_TIMEOUT, async {
                    let mut socket = match launch::connect(&endpoint).await {
                        Ok(socket) => socket,
                        Err(launch::GuestConnectError::Http(502)) => {
                            return Err(RpcAttemptError::EndpointApplicationUnavailable);
                        }
                        Err(_) => {
                            return Err(
                                temporary("could not connect to the physical sandbox").into()
                            );
                        }
                    };
                    socket
                        .send(Message::Text(encoded.clone().into()))
                        .await
                        .map_err(|error| {
                            temporary_from("could not send the Environment request", error)
                        })?;
                    while let Some(message) = socket.next().await {
                        let text = match message {
                            Ok(Message::Text(text)) => text.to_string(),
                            Ok(Message::Binary(bytes)) => String::from_utf8(bytes.to_vec())
                                .map_err(|error| {
                                    temporary_from("Environment response is not UTF-8", error)
                                })?,
                            Ok(Message::Ping(bytes)) => {
                                socket.send(Message::Pong(bytes)).await.map_err(|error| {
                                    temporary_from("Environment ping response failed", error)
                                })?;
                                continue;
                            }
                            Ok(Message::Pong(_)) => continue,
                            Ok(Message::Close(_)) | Err(_) => {
                                return Err(
                                    temporary("Environment connection ended before its receipt").into()
                                );
                            }
                            Ok(Message::Frame(_)) => continue,
                        };
                        let response: ResponseFrame = serde_json::from_str(&text)
                            .map_err(|error| temporary_from("Environment response is malformed", error))?;
                        if response.request_id != frame.request_id {
                            continue;
                        }
                        return match response.result {
                            // A reply variant that does not answer this request's method is a
                            // protocol contract violation, not a transient fault: replaying it
                            // reproduces the exact mismatch.
                            Ok(reply) if reply.method() != frame.call.method() => {
                                Err(RpcAttemptError::Environment(error(
                                    EnvironmentErrorCode::InvalidRequest,
                                    false,
                                    format!(
                                        "guest answered {} with a {} reply",
                                        frame.call.method(),
                                        reply.method()
                                    ),
                                )))
                            }
                            Ok(reply) => Ok(reply),
                            Err(refusal) => Err(refusal.into()),
                        };
                    }
                    Err(temporary("Environment connection ended before its receipt").into())
                })
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(RpcAttemptError::Environment(temporary("Environment request timed out"))),
                };
            match result {
                Ok(reply) => return Ok(reply),
                Err(RpcAttemptError::EndpointApplicationUnavailable) => {
                    if record_application_failure(&mut saw_application_unavailable, attempt) {
                        return Err(endpoint_application_gone());
                    }
                    if attempt == 0 {
                        self.forget(&target.target_ref).await;
                    } else {
                        return Err(temporary("physical sandbox endpoint is unavailable"));
                    }
                }
                Err(RpcAttemptError::Environment(error)) if attempt == 0 && error.retryable => {
                    self.forget(&target.target_ref).await;
                }
                Err(RpcAttemptError::Environment(error)) => return Err(error),
            }
        }
        unreachable!("bounded retry loop returns")
    }

    pub async fn post_json<T: serde::Serialize>(
        &self,
        target: &InstalledTarget,
        path: &str,
        body: &T,
    ) -> Result<(), EnvironmentError> {
        let bytes = serde_json::to_vec(body).map_err(|_| {
            error(
                EnvironmentErrorCode::InvalidRequest,
                false,
                "install body is invalid",
            )
        })?;
        self.post_bytes(target, path, bytes, "application/json")
            .await
    }

    pub async fn post_blob<T: serde::Serialize>(
        &self,
        target: &InstalledTarget,
        path: &str,
        metadata: &T,
        bytes: &[u8],
    ) -> Result<(), EnvironmentError> {
        let body = encode_blob_install(metadata, bytes)?;
        self.post_bytes(target, path, body, "application/octet-stream")
            .await
    }

    pub async fn post_file<T: serde::Serialize>(
        &self,
        target: &InstalledTarget,
        path: &str,
        metadata: &T,
        file_path: &Path,
        bytes: u64,
    ) -> Result<(), EnvironmentError> {
        let metadata = serde_json::to_vec(metadata).map_err(|_| {
            error(
                EnvironmentErrorCode::InvalidRequest,
                false,
                "object metadata is invalid",
            )
        })?;
        let metadata = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(metadata);
        self.endpoint_request(target, "guest object install", |endpoint: LaunchedEnvironment| {
            let metadata = metadata.clone();
            async move {
                let file = tokio::fs::File::open(file_path).await.map_err(|error| {
                    EndpointAttemptError::Fatal(temporary_from(
                        "staged object is unavailable",
                        error,
                    ))
                })?;
                let body = reqwest::Body::wrap_stream(ReaderStream::new(
                    file.take(bytes.saturating_add(1)),
                ));
                self.http
                    .post(format!("{}{path}", endpoint.endpoint))
                    .header(AUTH_HEADER, &endpoint.auth_token)
                    .header(CONTROL_AUTH_HEADER, target.control_token.expose())
                    .header(OBJECT_METADATA_HEADER, &metadata)
                    .header(reqwest::header::CONTENT_LENGTH, bytes)
                    .body(body)
                    .timeout(Duration::from_secs(15 * 60))
                    .send()
                    .await
                    .map_err(EndpointAttemptError::Transport)
            }
        })
        .await
        .map(|_| ())
    }

    pub async fn export_file(
        &self,
        target: &InstalledTarget,
        request: &brain_protocol::environment::SandboxFileRequest,
    ) -> Result<(brain_protocol::environment::FileEntry, reqwest::Response), EnvironmentError> {
        let encoded = serde_json::to_vec(request).map_err(|_| {
            error(
                EnvironmentErrorCode::InvalidRequest,
                false,
                "file export is invalid",
            )
        })?;
        let response = self
            .endpoint_request(target, "guest file export", |endpoint: LaunchedEnvironment| {
                let encoded = encoded.clone();
                async move {
                    self.http
                        .post(format!("{}/internal/files/export", endpoint.endpoint))
                        .header(AUTH_HEADER, &endpoint.auth_token)
                        .header(CONTROL_AUTH_HEADER, target.control_token.expose())
                        .header("content-type", "application/json")
                        .body(encoded.clone())
                        .timeout(Duration::from_secs(15 * 60))
                        .send()
                        .await
                        .map_err(EndpointAttemptError::Transport)
                }
            })
            .await?;
        let metadata = response
            .headers()
            .get(FILE_ENTRY_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(value)
                    .ok()
            })
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| temporary("guest file metadata is malformed"))?;
        Ok((metadata, response))
    }

    async fn post_bytes(
        &self,
        target: &InstalledTarget,
        path: &str,
        body: Vec<u8>,
        content_type: &str,
    ) -> Result<(), EnvironmentError> {
        if body.len() > MAX_INSTALL_BODY_BYTES {
            return Err(error(
                EnvironmentErrorCode::ResourceExhausted,
                false,
                "install body exceeds the guest bound",
            ));
        }
        self.endpoint_request(target, "guest install", |endpoint: LaunchedEnvironment| {
            let body = body.clone();
            async move {
                self.http
                    .post(format!("{}{path}", endpoint.endpoint))
                    .header(AUTH_HEADER, &endpoint.auth_token)
                    .header(CONTROL_AUTH_HEADER, target.control_token.expose())
                    .header("content-type", content_type)
                    .body(body)
                    .timeout(RPC_TIMEOUT)
                    .send()
                    .await
                    .map_err(EndpointAttemptError::Transport)
            }
        })
        .await
        .map(|_| ())
    }

    /// One bounded exact-replay ladder for every HTTP request against the guest endpoint: refresh
    /// the cached JWE once on 401/5xx/transport failure, fence the generation on repeated
    /// application 502, and classify everything else exactly once.
    async fn endpoint_request<F, Fut>(
        &self,
        target: &InstalledTarget,
        operation: &'static str,
        send: F,
    ) -> Result<reqwest::Response, EnvironmentError>
    where
        F: Fn(LaunchedEnvironment) -> Fut,
        Fut: std::future::Future<Output = Result<reqwest::Response, EndpointAttemptError>> + Send,
    {
        let mut saw_application_unavailable = false;
        for attempt in 0..2 {
            let endpoint = self.endpoint(target).await?;
            match send(endpoint).await {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response) if response.status().as_u16() == 401 && attempt == 0 => {
                    self.forget(&target.target_ref).await;
                }
                Ok(response) if response.status().as_u16() == 502 => {
                    if record_application_failure(&mut saw_application_unavailable, attempt) {
                        return Err(endpoint_application_gone());
                    }
                    if attempt == 0 {
                        self.forget(&target.target_ref).await;
                    } else {
                        return Err(temporary("physical sandbox endpoint is unavailable"));
                    }
                }
                Ok(response) if response.status().is_server_error() && attempt == 0 => {
                    self.forget(&target.target_ref).await;
                }
                Ok(response) => {
                    return Err(endpoint_response_error(response.status(), operation));
                }
                Err(EndpointAttemptError::Fatal(error)) => return Err(error),
                Err(EndpointAttemptError::Transport(error)) if attempt == 0 => {
                    tracing::debug!(%error, operation, "guest endpoint attempt failed; refreshing");
                    self.forget(&target.target_ref).await;
                }
                Err(EndpointAttemptError::Transport(error)) => {
                    return Err(temporary_from(operation, error));
                }
            }
        }
        unreachable!("bounded retry loop returns")
    }
}

/// One request attempt against the guest endpoint: a transport failure is retried once with a
/// refreshed endpoint, while a fatal failure already carries its exact classification.
enum EndpointAttemptError {
    Transport(reqwest::Error),
    Fatal(EnvironmentError),
}

fn encode_blob_install<T: serde::Serialize>(
    metadata: &T,
    bytes: &[u8],
) -> Result<Vec<u8>, EnvironmentError> {
    let mut body = serde_json::to_vec(metadata).map_err(|_| {
        error(
            EnvironmentErrorCode::InvalidRequest,
            false,
            "install metadata is invalid",
        )
    })?;
    if body.len() > MAX_INSTALL_METADATA_BYTES || bytes.len() > MAX_BUNDLE_INSTALL_BYTES {
        return Err(error(
            EnvironmentErrorCode::ResourceExhausted,
            false,
            "bundle or immutable metadata exceeds the guest install bound",
        ));
    }
    body.reserve_exact(bytes.len().saturating_add(1));
    body.push(b'\n');
    body.extend_from_slice(bytes);
    debug_assert!(body.len() <= MAX_INSTALL_BODY_BYTES);
    Ok(body)
}

fn endpoint_application_gone() -> EnvironmentError {
    error(
        EnvironmentErrorCode::SandboxGone,
        false,
        "physical sandbox application is persistently unavailable",
    )
}

fn record_application_failure(seen: &mut bool, attempt: usize) -> bool {
    let persistent = *seen && attempt > 0;
    *seen = true;
    persistent
}

fn endpoint_response_error(status: reqwest::StatusCode, operation: &str) -> EnvironmentError {
    if status.is_server_error() {
        temporary(format!("{operation} is temporarily unavailable"))
    } else {
        error(
            EnvironmentErrorCode::InvalidRequest,
            false,
            format!("{operation} refused with HTTP {status}"),
        )
    }
}

/// One classification for every provider control failure. The provider's own message text stays
/// out of the public Environment contract; scope and pacing survive as structured details.
pub(crate) fn control_error(error_value: ControlError) -> EnvironmentError {
    match error_value {
        ControlError::Gone(_) => error(EnvironmentErrorCode::SandboxGone, false, "sandbox is gone"),
        ControlError::Capacity {
            scope,
            retry_after_ms,
            ..
        } => {
            let mut value = error(
                EnvironmentErrorCode::ResourceExhausted,
                true,
                "sandbox provider capacity is exhausted",
            );
            value.details.insert("scope".into(), scope.into());
            value
                .details
                .insert("retry_after_ms".into(), retry_after_ms.into());
            value
        }
        ControlError::Retryable(_) | ControlError::Throttled(_) | ControlError::Unknown(_) => {
            temporary("sandbox provider is temporarily unavailable")
        }
        ControlError::Fatal(_) => error(
            EnvironmentErrorCode::CapabilityUnavailable,
            false,
            "sandbox provider configuration is invalid",
        ),
    }
}

pub fn error(code: EnvironmentErrorCode, retryable: bool, message: impl Into<String>) -> EnvironmentError {
    EnvironmentError {
        code,
        details: serde_json::Map::new(),
        message: message
            .into()
            .parse()
            .unwrap_or_else(|_| "Environment request failed".parse().expect("fallback message")),
        retryable,
    }
}

pub fn temporary(message: impl Into<String>) -> EnvironmentError {
    error(EnvironmentErrorCode::TemporarilyUnavailable, true, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_repeated_endpoint_502_fences_the_physical_generation() {
        let mut seen = false;
        assert!(!record_application_failure(&mut seen, 0));
        assert!(record_application_failure(&mut seen, 1));

        let mut only_second_attempt_failed = false;
        assert!(!record_application_failure(
            &mut only_second_attempt_failed,
            1
        ));
        let transient =
            endpoint_response_error(reqwest::StatusCode::SERVICE_UNAVAILABLE, "guest request");
        assert_eq!(transient.code, EnvironmentErrorCode::TemporarilyUnavailable);
        assert!(transient.retryable);
        let gone = endpoint_application_gone();
        assert_eq!(gone.code, EnvironmentErrorCode::SandboxGone);
        assert!(!gone.retryable);
    }

    #[test]
    fn private_install_transport_accepts_session_bundle_ceiling_and_rejects_plus_one() {
        let exact = vec![0x5a; MAX_BUNDLE_INSTALL_BYTES];
        let encoded = encode_blob_install(&serde_json::json!({}), &exact).unwrap();
        assert_eq!(encoded.len(), MAX_BUNDLE_INSTALL_BYTES + 3);
        assert!(encoded.len() <= MAX_INSTALL_BODY_BYTES);

        let oversized = vec![0x5a; MAX_BUNDLE_INSTALL_BYTES + 1];
        let error = encode_blob_install(&serde_json::json!({}), &oversized).unwrap_err();
        assert_eq!(error.code, EnvironmentErrorCode::ResourceExhausted);
        assert!(!error.retryable);
    }
}
