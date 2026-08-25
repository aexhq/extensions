//! Bounded WebSocket transport plus provider and immutable-install routes.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use brain_protocol::contract::ENVIRONMENT_CONTRACT_DIGEST;
use environment_wire::{
    CONTROL_AUTH_HEADER, FILE_ENTRY_HEADER, InstallBindingRequest, InstallBundleMetadata,
    InstallObjectMetadata, InstallSecretsRequest, MAX_INSTALL_BODY_BYTES, MAX_OBJECT_BYTES,
    MAX_WIRE_FRAME_BYTES, OBJECT_METADATA_HEADER, RequestCall, RequestFrame, ResponseFrame,
    ResponseReply,
};
use futures_util::StreamExt;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio_util::io::ReaderStream;

use crate::environment::Environment;
use crate::errors::{environment_error, invalid, status_for};
use crate::hooks;

const CONTROL_AUTH_PROTOCOL_PREFIX: &str = "aex-environment-control.";

pub struct Server {
    listener: TcpListener,
    environment: Arc<Environment>,
}

impl Server {
    pub async fn bind(environment: Arc<Environment>) -> anyhow::Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(environment.cfg.listen).await?,
            environment,
        })
    }

    pub fn local_addr(&self) -> anyhow::Result<SocketAddr> {
        Ok(self.listener.local_addr()?)
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let hooks = Router::new()
            .route("/run", post(hooks::run))
            .route("/ready", post(hooks::ready))
            .route("/validate", post(hooks::validate));
        let protected = Router::new()
            .route("/", get(root))
            .route("/internal/bundles/{digest}", post(install_bundle))
            .route("/internal/objects/{digest}", post(install_object))
            .route("/internal/files/export", post(export_file))
            .route("/internal/bindings", post(install_binding))
            .route("/internal/secrets", post(install_secrets))
            .layer(middleware::from_fn_with_state(
                self.environment.clone(),
                require_control,
            ));
        let app = protected
            .nest(hooks::HOOK_PREFIX, hooks)
            .layer(DefaultBodyLimit::max(MAX_INSTALL_BODY_BYTES))
            .with_state(self.environment);
        use axum::serve::ListenerExt as _;
        let listener = self.listener.tap_io(|io| {
            let _ = io.set_nodelay(true);
        });
        axum::serve(listener, app).await?;
        Ok(())
    }
}

async fn require_control(
    State(environment): State<Arc<Environment>>,
    request: Request,
    next: Next,
) -> Response {
    let candidate = control_candidate(request.headers());
    if !environment.control_authorized(candidate).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

fn control_candidate(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(CONTROL_AUTH_HEADER)
        .and_then(|value| value.to_str().ok())
        .or_else(|| {
            headers
                .get(header::SEC_WEBSOCKET_PROTOCOL)
                .and_then(|value| value.to_str().ok())
                .and_then(|protocols| {
                    protocols
                        .split(',')
                        .map(str::trim)
                        .find_map(|protocol| protocol.strip_prefix(CONTROL_AUTH_PROTOCOL_PREFIX))
                })
        })
}

fn control_protocol(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .and_then(|protocols| {
            protocols
                .split(',')
                .map(str::trim)
                .find(|protocol| protocol.starts_with(CONTROL_AUTH_PROTOCOL_PREFIX))
        })
        .map(str::to_owned)
}

struct MaybeWs(Option<WebSocketUpgrade>);

impl<S: Send + Sync> FromRequestParts<S> for MaybeWs {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            WebSocketUpgrade::from_request_parts(parts, state)
                .await
                .ok(),
        ))
    }
}

async fn root(
    State(environment): State<Arc<Environment>>,
    headers: HeaderMap,
    MaybeWs(ws): MaybeWs,
) -> axum::response::Response {
    match ws {
        Some(upgrade) => {
            let upgrade = match control_protocol(&headers) {
                Some(protocol) => upgrade.protocols([protocol]),
                None => upgrade,
            };
            upgrade
                .max_message_size(MAX_WIRE_FRAME_BYTES)
                .max_frame_size(MAX_WIRE_FRAME_BYTES)
                .on_upgrade(move |socket| serve_connection(environment, socket))
        }
        None => Json(serde_json::json!({
            "service": "environment",
            "contract_digest": ENVIRONMENT_CONTRACT_DIGEST.trim(),
            "target": environment.runtime_status().await,
        }))
        .into_response(),
    }
}

async fn serve_connection(environment: Arc<Environment>, mut socket: WebSocket) {
    let mut canary_exit_armed = false;
    while let Some(message) = socket.next().await {
        let text = match message {
            Ok(Message::Text(text)) => text.to_string(),
            Ok(Message::Binary(bytes)) => match String::from_utf8(bytes.to_vec()) {
                Ok(text) => text,
                Err(_) => continue,
            },
            Ok(Message::Ping(bytes)) => {
                let _ = socket.send(Message::Pong(bytes)).await;
                continue;
            }
            Ok(Message::Pong(_)) => continue,
            Ok(Message::Close(_)) | Err(_) => break,
        };
        let frame = match serde_json::from_str::<RequestFrame>(&text) {
            Ok(frame) => frame,
            Err(error) => {
                // No decodable request_id means no addressable error frame; at least be visible
                // instead of letting the caller block until its own timeout.
                tracing::warn!(%error, "undecodable request frame was dropped");
                continue;
            }
        };
        if canary_exit_armed {
            // The publisher sends this second frame only after receiving the acknowledgement, so
            // the deliberate crash has a protocol ordering boundary instead of a proxy delay.
            std::process::abort();
        }
        let request_id = frame.request_id.clone();
        let arm_canary_exit = match &frame.call {
            RequestCall::AcknowledgeTerminal(request) => {
                environment
                    .should_exit_for_canary_operation(request.operation.operation_id.as_str())
                    .await
            }
            _ => false,
        };
        let result = if frame.contract_digest != ENVIRONMENT_CONTRACT_DIGEST.trim() {
            Err(invalid("Environment contract digest mismatch"))
        } else {
            dispatch(&environment, frame.call).await
        };
        let arm_canary_exit = arm_canary_exit && result.is_ok();
        let response = ResponseFrame { request_id, result };
        match serde_json::to_string(&response) {
            Ok(text) if text.len() <= MAX_WIRE_FRAME_BYTES => {
                match terminal_operation_id(&response) {
                    Some(operation_id)
                        if environment
                            .should_exit_for_canary_operation(operation_id)
                            .await =>
                    {
                        if let Err(error) =
                            commit_canary_terminal_receipt(&environment, text.as_bytes()).await
                        {
                            tracing::error!(%error, "image canary terminal receipt sync failed");
                        }
                    }
                    _ => {}
                }
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
                canary_exit_armed = arm_canary_exit;
            }
            // A response one byte over the frame bound must surface as an addressable error, not
            // as a silent connection drop the client can only read as Environment loss.
            _ => {
                let refusal = ResponseFrame {
                    request_id: response.request_id,
                    result: Err(environment_error(
                        brain_protocol::environment::EnvironmentErrorCode::ResourceExhausted,
                        false,
                        "response exceeded the wire frame bound and was withheld",
                    )),
                };
                match serde_json::to_string(&refusal) {
                    Ok(text) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        tracing::error!(%error, "error frame did not serialize; closing");
                        break;
                    }
                }
            }
        }
    }
}

fn terminal_operation_id(response: &ResponseFrame) -> Option<&str> {
    let observation = match response.result.as_ref().ok()? {
        ResponseReply::Submit(receipt) | ResponseReply::ExecuteSandbox(receipt) => {
            &receipt.observation
        }
        ResponseReply::Observe(observation) => observation,
        _ => return None,
    };
    observation
        .terminal
        .as_ref()
        .map(|_| observation.operation.operation_id.as_str())
}

async fn commit_canary_terminal_receipt(
    environment: &Environment,
    bytes: &[u8],
) -> std::io::Result<()> {
    let temporary = environment.cfg.state_dir.join(".image-canary-terminal.tmp");
    let committed = environment.cfg.state_dir.join("image-canary-terminal.json");
    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).await?;
    file.write_all(bytes).await?;
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&temporary, &committed).await?;
    #[cfg(unix)]
    {
        let directory = environment.cfg.state_dir.clone();
        tokio::task::spawn_blocking(move || std::fs::File::open(directory)?.sync_all())
            .await
            .map_err(std::io::Error::other)??;
    }
    Ok(())
}

async fn dispatch(
    environment: &Arc<Environment>,
    call: RequestCall,
) -> Result<ResponseReply, brain_protocol::environment::EnvironmentError> {
    match call {
        RequestCall::Submit(request) => environment
            .submit(*request)
            .await
            .map(ResponseReply::Submit),
        RequestCall::Observe(request) => environment
            .observe(request)
            .await
            .map(ResponseReply::Observe),
        RequestCall::Cancel(request) => {
            environment.cancel(request).await.map(ResponseReply::Cancel)
        }
        RequestCall::AcknowledgeTerminal(request) => environment
            .acknowledge_terminal(request)
            .await
            .map(ResponseReply::AcknowledgeTerminal),
        RequestCall::Status => environment
            .runtime_status()
            .await
            .map(ResponseReply::Status)
            .ok_or_else(|| invalid("target is not armed")),
        RequestCall::ListFiles(request) => environment
            .list_files(request)
            .await
            .map(ResponseReply::ListFiles),
        RequestCall::StatFile(request) => environment
            .stat_file(request)
            .await
            .map(ResponseReply::StatFile),
        RequestCall::ReadFile(request) => environment
            .read_file(request)
            .await
            .map(ResponseReply::ReadFile),
        RequestCall::WriteFile(request) => environment
            .write_file(request)
            .await
            .map(ResponseReply::WriteFile),
        RequestCall::ReserveFileEffect(identity) => environment
            .reserve_file_effect(identity)
            .await
            .map(ResponseReply::ReserveFileEffect),
        RequestCall::ClaimFileEffect(identity) => environment
            .claim_file_effect(identity)
            .await
            .map(ResponseReply::ClaimFileEffect),
        RequestCall::CompleteFileEffect(result) => environment
            .complete_file_effect(result)
            .await
            .map(ResponseReply::CompleteFileEffect),
        RequestCall::FindFiles(request) => environment
            .find_files(request)
            .await
            .map(ResponseReply::FindFiles),
        RequestCall::GrepFiles(request) => environment
            .grep_files(request)
            .await
            .map(ResponseReply::GrepFiles),
        RequestCall::ExecuteSandbox(request) => environment
            .execute_sandbox(request)
            .await
            .map(ResponseReply::ExecuteSandbox),
        RequestCall::WriteStdin(request) => environment
            .write_stdin(request)
            .await
            .map(ResponseReply::WriteStdin),
    }
}

/// Body is one metadata JSON line followed by exact immutable bundle bytes.
async fn install_bundle(
    State(environment): State<Arc<Environment>>,
    Path(digest): Path<String>,
    bytes: Bytes,
) -> impl IntoResponse {
    let metadata_bytes = match bytes.iter().position(|byte| *byte == b'\n') {
        Some(boundary) => &bytes[..boundary],
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing metadata"})),
            );
        }
    };
    let metadata: InstallBundleMetadata = match serde_json::from_slice(metadata_bytes) {
        Ok(metadata) => metadata,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid metadata"})),
            );
        }
    };
    if metadata.layer_digest != digest {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "digest mismatch"})),
        );
    }
    let content = &bytes[metadata_bytes.len() + 1..];
    reply(environment.install_bundle(metadata, content).await)
}

async fn install_binding(
    State(environment): State<Arc<Environment>>,
    Json(request): Json<InstallBindingRequest>,
) -> impl IntoResponse {
    reply(environment.install_binding(request).await)
}

async fn install_object(
    State(environment): State<Arc<Environment>>,
    Path(digest): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> axum::response::Response {
    let metadata: InstallObjectMetadata = match headers
        .get(OBJECT_METADATA_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(value)
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    {
        Some(metadata) => metadata,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid metadata"})),
            )
                .into_response();
        }
    };
    if metadata.object.sha256.as_str() != digest {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "digest mismatch"})),
        )
            .into_response();
    }
    if metadata.object.bytes > MAX_OBJECT_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({"error": "object too large"})),
        )
            .into_response();
    }
    if headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|bytes| bytes != metadata.object.bytes)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "length mismatch"})),
        )
            .into_response();
    }
    let temporary = environment.cfg.object_dir.join(format!(
        ".{digest}.install-{}",
        hex::encode(rand::random::<[u8; 16]>())
    ));
    let mut options = tokio::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = match options.open(&temporary).await {
        Ok(file) => file,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "staging unavailable"})),
            )
                .into_response();
        }
    };
    let mut count = 0u64;
    let mut hash = Sha256::new();
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(_) => {
                let _ = tokio::fs::remove_file(&temporary).await;
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "object stream failed"})),
                )
                    .into_response();
            }
        };
        count = count.saturating_add(chunk.len() as u64);
        if count > metadata.object.bytes || count > MAX_OBJECT_BYTES {
            let _ = tokio::fs::remove_file(&temporary).await;
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(serde_json::json!({"error": "object exceeds sealed size"})),
            )
                .into_response();
        }
        hash.update(&chunk);
        if file.write_all(&chunk).await.is_err() {
            let _ = tokio::fs::remove_file(&temporary).await;
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "staging failed"})),
            )
                .into_response();
        }
    }
    if file.flush().await.is_err() || file.sync_all().await.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "staging sync failed"})),
        )
            .into_response();
    }
    drop(file);
    reply(
        environment
            .install_object_file(metadata, temporary, count, &hex::encode(hash.finalize()))
            .await,
    )
    .into_response()
}

async fn export_file(
    State(environment): State<Arc<Environment>>,
    Json(request): Json<brain_protocol::environment::SandboxFileRequest>,
) -> axum::response::Response {
    let (entry, file) = match environment.open_file_export(request).await {
        Ok(value) => value,
        Err(error) => return reply::<serde_json::Value>(Err(error)).into_response(),
    };
    if entry.bytes > MAX_OBJECT_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({"error": "file exceeds object bound"})),
        )
            .into_response();
    }
    let metadata = match serde_json::to_vec(&entry) {
        Ok(value) => base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let file = tokio::fs::File::from_std(file).take(MAX_OBJECT_BYTES.saturating_add(1));
    match axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(FILE_ENTRY_HEADER, metadata)
        .body(Body::from_stream(ReaderStream::new(file)))
    {
        Ok(response) => response,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn install_secrets(
    State(environment): State<Arc<Environment>>,
    Json(request): Json<InstallSecretsRequest>,
) -> impl IntoResponse {
    reply(environment.install_secrets(request).await)
}

fn reply<T: serde::Serialize>(
    result: Result<T, brain_protocol::environment::EnvironmentError>,
) -> (StatusCode, Json<serde_json::Value>) {
    match result {
        Ok(value) => (
            StatusCode::OK,
            Json(serde_json::to_value(value).expect("install receipt serializes")),
        ),
        Err(error) => (
            status_for(error.code),
            Json(serde_json::json!({"error": error.message.as_str(), "code": error.code})),
        ),
    }
}
