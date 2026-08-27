use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use brain_protocol_current::{EnvironmentBinding, EnvironmentRequest, ToolInvocation};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::net::IpAddr;
use std::time::Duration;
use tokio::sync::Mutex;

mod aws;

pub use aws::AwsDriver;

pub const MAX_ENVIRONMENT_REQUEST_BYTES: usize = 2 * 1024 * 1024;
const MAX_OPERATION_IDENTITIES: usize = 100_000;
const MAX_TOOL_BUNDLE_BYTES: usize = 8 * 1024 * 1024;
const ENVIRONMENT_CONTRACT: &str = "environment/v1";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchRequest {
    pub operation_id: String,
    pub action: String,
    pub request: Value,
    pub deadline_at_ms: String,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct DriverError {
    pub status: StatusCode,
    pub message: String,
}

impl DriverError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait Driver: Send + Sync {
    async fn dispatch(&self, request: DispatchRequest) -> Result<Value, DriverError>;
}

#[derive(Clone)]
pub struct HttpRelayDriver {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    bearer: Option<String>,
    timeout: Duration,
}

impl HttpRelayDriver {
    pub fn new(
        endpoint: impl Into<String>,
        bearer: Option<String>,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(!timeout.is_zero(), "relay timeout must be positive");
        let endpoint = endpoint.into().parse::<reqwest::Url>()?;
        let loopback_http = endpoint.scheme() == "http"
            && endpoint
                .host_str()
                .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback());
        anyhow::ensure!(
            endpoint.scheme() == "https" || loopback_http,
            "relay URL must use HTTPS or literal loopback HTTP"
        );
        anyhow::ensure!(
            endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.query().is_none()
                && endpoint.fragment().is_none(),
            "relay URL cannot contain credentials, query, or fragment"
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(timeout.min(Duration::from_secs(10)))
            .build()?;
        Ok(Self {
            client,
            endpoint,
            bearer,
            timeout,
        })
    }
}

#[async_trait]
impl Driver for HttpRelayDriver {
    async fn dispatch(&self, request: DispatchRequest) -> Result<Value, DriverError> {
        let deadline_at_ms = request
            .deadline_at_ms
            .parse::<u64>()
            .map_err(|_| DriverError::invalid("dispatch deadline is invalid"))?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let remaining = deadline_at_ms.saturating_sub(now_ms);
        if remaining == 0 {
            return Err(DriverError::unavailable("dispatch deadline elapsed"));
        }
        let mut builder = self
            .client
            .post(self.endpoint.clone())
            .header(header::CONTENT_TYPE, "application/json")
            .json(&request)
            .timeout(self.timeout.min(Duration::from_millis(remaining)));
        if let Some(bearer) = &self.bearer {
            builder = builder.bearer_auth(bearer);
        }
        let response = builder
            .send()
            .await
            .map_err(|_| DriverError::unavailable("Environment relay is unavailable"))?;
        let status = response.status();
        const MAX_RELAY_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
        if !status.is_success() {
            // Collapsing every client error into one status, and dropping the relayed body, left a
            // caller with "400 Bad Request" and no way to tell a refusal from a bad credential.
            let detail = response.text().await.unwrap_or_default();
            let detail = detail.trim();
            return Err(DriverError {
                status,
                message: if detail.is_empty() {
                    format!("Environment relay returned {status}")
                } else {
                    format!(
                        "Environment relay returned {status}: {}",
                        detail.chars().take(512).collect::<String>()
                    )
                },
            });
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|_| DriverError::unavailable("Environment relay response failed"))?;
            if body.len().saturating_add(chunk.len()) > MAX_RELAY_RESPONSE_BYTES {
                return Err(DriverError::invalid(
                    "Environment relay response is too large",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body)
            .map_err(|_| DriverError::invalid("Environment relay returned invalid JSON"))
    }
}

#[derive(Clone)]
struct DriverState {
    bearer_digest: [u8; 32],
    drivers: Arc<HashMap<String, Arc<dyn Driver>>>,
    tools: Arc<HashMap<String, ToolBundle>>,
    operation_digests: Arc<Mutex<OperationIdentityBook>>,
}

#[derive(Clone)]
struct ToolBundle {
    contract_digest: String,
    bundle_digest: String,
    bytes: Arc<[u8]>,
}

#[derive(Clone)]
struct ActiveEnvironment {
    driver: String,
    provider_configuration: Value,
    configuration_digest: String,
}

#[derive(Default)]
struct OperationIdentityBook {
    by_id: HashMap<String, String>,
    order: VecDeque<String>,
}

#[derive(Deserialize)]
struct ToolRegistryEntry {
    contract_digest: String,
    filename: String,
}

type EnvironmentCommand = brain_protocol_current::EnvironmentCommand<EnvironmentRequest>;
type EnvironmentOperation = brain_protocol_current::EnvironmentOperation<EnvironmentRequest>;

pub fn router(
    bearer: impl Into<String>,
    drivers: impl IntoIterator<Item = (String, Arc<dyn Driver>)>,
    tool_directory: impl AsRef<Path>,
) -> anyhow::Result<Router> {
    let bearer = bearer.into();
    anyhow::ensure!(
        !bearer.is_empty(),
        "Environment driver token cannot be empty"
    );
    let mut by_name = HashMap::new();
    for (name, driver) in drivers {
        anyhow::ensure!(
            !name.is_empty() && name.len() <= 64,
            "Environment driver name is invalid"
        );
        anyhow::ensure!(
            by_name.insert(name.clone(), driver).is_none(),
            "Environment driver {name} is registered more than once"
        );
    }
    anyhow::ensure!(
        !by_name.is_empty(),
        "at least one Environment driver is required"
    );
    let tools = load_tools(tool_directory.as_ref())?;
    let state = DriverState {
        bearer_digest: Sha256::digest(bearer.as_bytes()).into(),
        drivers: Arc::new(by_name),
        tools: Arc::new(tools),
        operation_digests: Arc::new(Mutex::new(OperationIdentityBook::default())),
    };
    Ok(Router::new()
        .route("/v1/operations", post(operation))
        .layer(DefaultBodyLimit::max(MAX_ENVIRONMENT_REQUEST_BYTES))
        .with_state(state))
}

fn load_tools(directory: &Path) -> anyhow::Result<HashMap<String, ToolBundle>> {
    let registry_bytes = std::fs::read(directory.join("registry.json"))?;
    anyhow::ensure!(
        registry_bytes.len() <= 256 * 1024,
        "Tool registry exceeds 256 KiB"
    );
    let registry: HashMap<String, ToolRegistryEntry> = serde_json::from_slice(&registry_bytes)?;
    anyhow::ensure!(!registry.is_empty(), "Tool registry cannot be empty");
    let canonical_directory = std::fs::canonicalize(directory)?;
    let mut tools = HashMap::with_capacity(registry.len());
    for (name, entry) in registry {
        anyhow::ensure!(valid_identifier(&name), "Tool registry name is invalid");
        anyhow::ensure!(
            entry.contract_digest.len() == 64
                && entry
                    .contract_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()),
            "Tool {name} contract digest is invalid"
        );
        let path = std::fs::canonicalize(directory.join(&entry.filename))?;
        anyhow::ensure!(
            path.parent() == Some(canonical_directory.as_path()),
            "Tool {name} bundle must be directly inside the Tool directory"
        );
        let bytes = std::fs::read(&path)?;
        anyhow::ensure!(
            !bytes.is_empty() && bytes.len() <= MAX_TOOL_BUNDLE_BYTES,
            "Tool {name} bundle size is invalid"
        );
        let bundle = ToolBundle {
            contract_digest: entry.contract_digest.to_ascii_lowercase(),
            bundle_digest: hex::encode(Sha256::digest(&bytes)),
            bytes: bytes.into(),
        };
        anyhow::ensure!(
            tools.insert(name.clone(), bundle).is_none(),
            "Tool {name} is registered more than once"
        );
    }
    Ok(tools)
}

async fn operation(State(state): State<DriverState>, headers: HeaderMap, body: Bytes) -> Response {
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| {
            constant_time_equal(&Sha256::digest(token.as_bytes()), &state.bearer_digest)
        });
    if !authorized {
        return failure(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let command: EnvironmentCommand = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "invalid Environment command"),
    };
    if command.contract != ENVIRONMENT_CONTRACT
        || !valid_operation(&command.operation)
        || !valid_binding(&command.binding, &command.operation)
    {
        return failure(StatusCode::BAD_REQUEST, "invalid Environment command");
    }
    let actual_digest = match canonical_digest(&command.operation.request) {
        Ok(digest) => digest,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "invalid Environment request"),
    };
    if actual_digest != command.operation.request_digest {
        return failure(
            StatusCode::BAD_REQUEST,
            "Environment request digest does not match its canonical request",
        );
    }
    let operation_id = command.operation.operation_id.clone();
    let request_digest = command.operation.request_digest.clone();
    if let Some(receipt) =
        record_operation_identity(&state, operation_id.as_str(), &request_digest).await
    {
        return environment_response(operation_id.as_str(), &request_digest, receipt);
    }
    let receipt = match handle_operation(&state, command.binding, command.operation).await {
        Ok(receipt) => receipt,
        Err(error) => {
            tracing::warn!(
                operation_id = %operation_id,
                status = %error.status,
                reason = %error.message,
                "Environment operation refused"
            );
            serde_json::json!({
                "type":"failure",
                "code": if error.status.is_server_error() {"unavailable"} else {"invalid_request"},
                "message": error.message,
                "retryable": error.status.is_server_error()
            })
        }
    };
    environment_response(operation_id.as_str(), &request_digest, receipt)
}

async fn record_operation_identity(
    state: &DriverState,
    operation_id: &str,
    request_digest: &str,
) -> Option<Value> {
    let mut operations = state.operation_digests.lock().await;
    if let Some(expected) = operations.by_id.get(operation_id) {
        return (expected != request_digest).then(|| {
            serde_json::json!({
                "type":"conflict",
                "expected_digest":expected,
                "actual_digest":request_digest
            })
        });
    }
    if operations.by_id.len() >= MAX_OPERATION_IDENTITIES
        && let Some(expired) = operations.order.pop_front()
    {
        operations.by_id.remove(&expired);
    }
    operations
        .by_id
        .insert(operation_id.to_owned(), request_digest.to_owned());
    operations.order.push_back(operation_id.to_owned());
    None
}

async fn handle_operation(
    state: &DriverState,
    binding: EnvironmentBinding,
    operation: EnvironmentOperation,
) -> Result<Value, DriverError> {
    let environment = environment_from_binding(state, &binding)?;
    match &operation.request {
        EnvironmentRequest::Setup { configuration } => setup(&environment, configuration),
        EnvironmentRequest::Attach { grants } => {
            let _ = grants;
            let expected = attachment_id(
                operation.session_id.as_str(),
                operation.environment_id.as_str(),
            )?;
            if operation.attachment_id.as_ref().map(|id| id.as_str()) != Some(expected.as_str()) {
                return Err(DriverError::invalid("invalid Environment attachment"));
            }
            Ok(serde_json::json!({"type":"accepted"}))
        }
        EnvironmentRequest::Execute {
            tool,
            remote_tool_id,
            grant,
        } => {
            execute(
                state,
                &environment,
                &operation,
                tool.clone(),
                remote_tool_id,
                grant.clone(),
            )
            .await
        }
        EnvironmentRequest::Call { name, input } => {
            let _ = (name, input);
            Err(DriverError::invalid(
                "AWS MicroVM Environment does not expose direct calls",
            ))
        }
        EnvironmentRequest::Cancel {
            target_operation_id,
        } => {
            let driver = state
                .drivers
                .get(&environment.driver)
                .cloned()
                .ok_or_else(|| {
                    DriverError::unavailable("Environment driver is no longer configured")
                })?;
            let deadline = now_ms().saturating_add(5_000);
            driver
                .dispatch(DispatchRequest {
                    operation_id: operation.operation_id.to_string(),
                    action: "cancel".into(),
                    request: serde_json::json!({
                        "binding":provider_binding(&environment, &operation),
                        "target_operation_id":target_operation_id
                    }),
                    deadline_at_ms: deadline.to_string(),
                })
                .await?;
            Ok(serde_json::json!({"type":"accepted"}))
        }
        EnvironmentRequest::Detach => {
            let expected = attachment_id(
                operation.session_id.as_str(),
                operation.environment_id.as_str(),
            )?;
            if operation.attachment_id.as_ref().map(|id| id.as_str()) != Some(expected.as_str()) {
                return Err(DriverError::invalid("invalid Environment attachment"));
            }
            Ok(serde_json::json!({"type":"accepted"}))
        }
        EnvironmentRequest::Teardown => teardown(state, &environment, &operation).await,
    }
}

fn setup(environment: &ActiveEnvironment, configuration: &Value) -> Result<Value, DriverError> {
    let actual = canonical_digest(configuration)?;
    if actual != environment.configuration_digest {
        return Ok(serde_json::json!({
            "type":"conflict",
            "expected_digest":environment.configuration_digest,
            "actual_digest":actual
        }));
    }
    Ok(serde_json::json!({"type":"accepted"}))
}

async fn execute(
    state: &DriverState,
    environment: &ActiveEnvironment,
    operation: &EnvironmentOperation,
    tool: ToolInvocation,
    remote_tool_id: &str,
    grant: Value,
) -> Result<Value, DriverError> {
    let expected = attachment_id(
        operation.session_id.as_str(),
        operation.environment_id.as_str(),
    )?;
    if operation.attachment_id.as_ref().map(|id| id.as_str()) != Some(expected.as_str()) {
        return Err(DriverError::invalid("invalid Environment attachment"));
    }
    if !valid_identifier(&tool.call_id) || !valid_identifier(&tool.name) {
        return Err(DriverError::invalid("invalid Tool invocation"));
    }
    let bundle = state.tools.get(remote_tool_id).ok_or_else(|| DriverError {
        status: StatusCode::NOT_FOUND,
        message: "remote Tool is not installed in this Environment".into(),
    })?;
    let driver = state
        .drivers
        .get(&environment.driver)
        .cloned()
        .ok_or_else(|| DriverError::unavailable("Environment driver is no longer configured"))?;
    let deadline = now_ms().saturating_add(120_000);
    let policy = grant
        .get("policy")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let binding = serde_json::json!({
        "driver":environment.driver,
        "configuration":environment.provider_configuration,
        "policy":policy,
        "tenant_id":operation.session_id,
        "session_id":operation.session_id,
        "root_id":operation.session_id,
        "parent_id":Value::Null,
        "environment_id":operation.environment_id
    });
    let submit = driver
        .dispatch(DispatchRequest {
            operation_id: operation.operation_id.to_string(),
            action: "submit".into(),
            request: serde_json::json!({
                "binding":binding,
                "operation":{
                    "operation_id":operation.operation_id,
                    "request_digest":operation.request_digest,
                    "kind":"invoke",
                    "descriptor_json":serde_json::to_string(&serde_json::json!({
                        "runtime":"node22",
                        "tool_name":remote_tool_id,
                        "contract_digest":bundle.contract_digest,
                        "bundle_digest":bundle.bundle_digest
                    })).map_err(|_| DriverError::unavailable("Tool descriptor could not be encoded"))?,
                    "bundle_base64":base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        bundle.bytes.as_ref()
                    ),
                    "input_json":serde_json::to_string(&tool.input)
                        .map_err(|_| DriverError::invalid("Tool input cannot be encoded"))?,
                    "deadline_at_ms":deadline.to_string()
                }
            }),
            deadline_at_ms: deadline.to_string(),
        })
        .await?;
    let provider_operation_id = submit
        .get("provider_operation_id")
        .and_then(Value::as_str)
        .ok_or_else(|| DriverError::unavailable("Environment returned no operation reference"))?;
    let mut cursor: Option<String> = None;
    loop {
        if now_ms() >= deadline {
            return Ok(serde_json::json!({
                "type":"ambiguous",
                "message":"Tool execution did not become terminal before its deadline"
            }));
        }
        let observation = driver
            .dispatch(DispatchRequest {
                operation_id: operation.operation_id.to_string(),
                action: "observe".into(),
                request: serde_json::json!({
                    "binding":binding,
                    "provider_operation_id":provider_operation_id,
                    "cursor":cursor
                }),
                deadline_at_ms: deadline.to_string(),
            })
            .await?;
        cursor = observation
            .get("cursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        match observation.get("state").and_then(Value::as_str) {
            Some("pending" | "running") => continue,
            Some("completed") => {
                let output = terminal_value(&observation)?;
                return Ok(serde_json::json!({
                    "type":"tool_result",
                    "result":{"call_id":tool.call_id,"output":output,"is_error":false}
                }));
            }
            Some("failed" | "cancelled") => {
                let output = terminal_value(&observation)?;
                return Ok(serde_json::json!({
                    "type":"tool_result",
                    "result":{"call_id":tool.call_id,"output":output,"is_error":true}
                }));
            }
            _ => {
                return Ok(serde_json::json!({
                    "type":"ambiguous",
                    "message":"Environment returned an unknown Tool operation state"
                }));
            }
        }
    }
}

async fn teardown(
    state: &DriverState,
    environment: &ActiveEnvironment,
    operation: &EnvironmentOperation,
) -> Result<Value, DriverError> {
    let driver = state
        .drivers
        .get(&environment.driver)
        .cloned()
        .ok_or_else(|| DriverError::unavailable("Environment driver is no longer configured"))?;
    let deadline = now_ms().saturating_add(30_000);
    driver
        .dispatch(DispatchRequest {
            operation_id: operation.operation_id.to_string(),
            action: "release".into(),
            request: serde_json::json!({
                "binding":{
                    "driver":environment.driver,
                    "configuration":environment.provider_configuration,
                    "policy":{},
                    "tenant_id":operation.session_id,
                    "session_id":operation.session_id,
                    "root_id":operation.session_id,
                    "parent_id":Value::Null,
                    "environment_id":operation.environment_id
                }
            }),
            deadline_at_ms: deadline.to_string(),
        })
        .await?;
    Ok(serde_json::json!({"type":"accepted"}))
}

fn environment_from_binding(
    state: &DriverState,
    binding: &EnvironmentBinding,
) -> Result<ActiveEnvironment, DriverError> {
    let configuration: Value = serde_json::from_str(&binding.adapter_binding)
        .map_err(|_| DriverError::invalid("Environment adapter binding is invalid"))?;
    let configuration_digest = canonical_digest(&configuration)?;
    if configuration_digest != binding.configuration_digest {
        return Err(DriverError::invalid(
            "Environment adapter binding does not match its digest",
        ));
    }
    let driver = configuration
        .get("driver")
        .and_then(Value::as_str)
        .ok_or_else(|| DriverError::invalid("Environment configuration has no driver"))?;
    if !state.drivers.contains_key(driver) {
        return Err(DriverError {
            status: StatusCode::NOT_FOUND,
            message: "Environment driver is not configured".into(),
        });
    }
    let mut provider_configuration = configuration.clone();
    provider_configuration
        .as_object_mut()
        .expect("a configuration containing driver is an object")
        .remove("driver");
    Ok(ActiveEnvironment {
        driver: driver.to_owned(),
        provider_configuration,
        configuration_digest,
    })
}

fn terminal_value(observation: &Value) -> Result<Value, DriverError> {
    observation
        .get("terminal_json")
        .and_then(Value::as_str)
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| DriverError::unavailable("Environment terminal result is invalid"))?
        .ok_or_else(|| DriverError::unavailable("Environment terminal result is missing"))
}

fn provider_binding(environment: &ActiveEnvironment, operation: &EnvironmentOperation) -> Value {
    serde_json::json!({
        "driver":environment.driver,
        "configuration":environment.provider_configuration,
        "policy":{},
        "tenant_id":operation.session_id,
        "session_id":operation.session_id,
        "root_id":operation.session_id,
        "parent_id":Value::Null,
        "environment_id":operation.environment_id
    })
}

fn attachment_id(session_id: &str, environment_id: &str) -> Result<String, DriverError> {
    let digest = canonical_digest(&(session_id, environment_id))?;
    Ok(format!("att_{}", &digest[..24]))
}

fn canonical_digest(value: &impl Serialize) -> Result<String, DriverError> {
    let bytes = serde_jcs::to_vec(value)
        .map_err(|_| DriverError::invalid("value cannot be canonicalized"))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn valid_operation(operation: &EnvironmentOperation) -> bool {
    valid_identifier(operation.operation_id.as_str())
        && operation.request_digest.len() == 64
        && operation
            .request_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        && valid_identifier(operation.environment_id.as_str())
        && valid_identifier(operation.session_id.as_str())
        && operation
            .attachment_id
            .as_ref()
            .map(|id| id.as_str())
            .is_none_or(valid_identifier)
}

fn valid_binding(binding: &EnvironmentBinding, operation: &EnvironmentOperation) -> bool {
    binding.environment_id == operation.environment_id
        && binding.configuration_digest.len() == 64
        && binding
            .configuration_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        && !binding.adapter_binding.is_empty()
        && binding.adapter_binding.len() <= 65_536
        && binding.directory_generation > 0
        && matches!(
            binding.lifecycle_policy,
            brain_protocol_current::LifecyclePolicy::Session
                | brain_protocol_current::LifecyclePolicy::Shared
                | brain_protocol_current::LifecyclePolicy::External
        )
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn environment_response(operation_id: &str, request_digest: &str, receipt: Value) -> Response {
    Json(serde_json::json!({
        "contract":ENVIRONMENT_CONTRACT,
        "operation_id":operation_id,
        "request_digest":request_digest,
        "receipt":receipt
    }))
    .into_response()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn failure(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Mutex as StdMutex;
    use tower::ServiceExt as _;

    struct FixtureDriver {
        actions: StdMutex<Vec<String>>,
        refuse: bool,
    }

    #[async_trait]
    impl Driver for FixtureDriver {
        async fn dispatch(&self, request: DispatchRequest) -> Result<Value, DriverError> {
            assert_eq!(
                request.request["binding"]["configuration"],
                serde_json::json!({})
            );
            self.actions.lock().unwrap().push(request.action.clone());
            if self.refuse {
                return Err(DriverError {
                    status: StatusCode::UNAUTHORIZED,
                    message: "Environment relay returned 401 Unauthorized: bad credential".into(),
                });
            }
            match request.action.as_str() {
                "submit" => Ok(serde_json::json!({"provider_operation_id":"provider-1"})),
                "observe" => Ok(serde_json::json!({
                    "state":"completed",
                    "cursor":"1",
                    "chunks":[],
                    "terminal_json":"{\"answer\":42}"
                })),
                "cancel" => Ok(serde_json::json!({})),
                "release" => Ok(serde_json::json!({})),
                _ => Err(DriverError::invalid("unexpected fixture action")),
            }
        }
    }

    fn tool_directory() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("echo.mjs"), "export default {};").unwrap();
        std::fs::write(
            directory.path().join("registry.json"),
            serde_json::json!({
                "echo":{"contract_digest":"a".repeat(64),"filename":"echo.mjs"}
            })
            .to_string(),
        )
        .unwrap();
        directory
    }

    fn command(
        token: &str,
        operation_id: &str,
        attachment_id: Option<&str>,
        operation: Value,
    ) -> Request<Body> {
        let configuration = serde_json::json!({"driver":"aws-microvm"});
        let configuration_digest = canonical_digest(&configuration).unwrap();
        let request_digest = canonical_digest(&operation).unwrap();
        Request::builder()
            .method("POST")
            .uri("/v1/operations")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({
                    "contract":"environment/v1",
                    "binding":{
                        "environment_id":"environment-1",
                        "configuration_digest":configuration_digest,
                        "adapter_binding":serde_jcs::to_string(&configuration).unwrap(),
                        "directory_generation":1,
                        "lifecycle_policy":"session"
                    },
                    "operation":{
                        "operation_id":operation_id,
                        "request_digest":request_digest,
                        "environment_id":"environment-1",
                        "session_id":"session-1",
                        "attachment_id":attachment_id,
                        "request":operation
                    }
                })
                .to_string(),
            ))
            .unwrap()
    }

    async fn body(response: Response) -> Value {
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn lifecycle_executes_an_installed_tool_and_tears_down() {
        let directory = tool_directory();
        let driver = Arc::new(FixtureDriver {
            actions: StdMutex::new(Vec::new()),
            refuse: false,
        });
        let first_task = router(
            "secret",
            [("aws-microvm".into(), driver.clone() as Arc<dyn Driver>)],
            directory.path(),
        )
        .unwrap();
        let second_task = router(
            "secret",
            [("aws-microvm".into(), driver.clone() as Arc<dyn Driver>)],
            directory.path(),
        )
        .unwrap();

        let setup = first_task
            .clone()
            .oneshot(command(
                "secret",
                "operation-setup",
                None,
                serde_json::json!({"type":"setup","configuration":{"driver":"aws-microvm"}}),
            ))
            .await
            .unwrap();
        assert_eq!(body(setup).await["receipt"]["type"], "accepted");

        let attachment = attachment_id("session-1", "environment-1").unwrap();
        let attach = second_task
            .clone()
            .oneshot(command(
                "secret",
                "operation-attach",
                Some(&attachment),
                serde_json::json!({"type":"attach","grants":{}}),
            ))
            .await
            .unwrap();
        assert_eq!(body(attach).await["receipt"]["type"], "accepted");

        let execute = second_task
            .clone()
            .oneshot(command(
                "secret",
                "operation-execute",
                Some(&attachment),
                serde_json::json!({
                    "type":"execute",
                    "tool":{"call_id":"call-1","name":"echo","input":{"value":42}},
                    "remote_tool_id":"echo",
                    "grant":{}
                }),
            ))
            .await
            .unwrap();
        let execute = body(execute).await;
        assert_eq!(execute["receipt"]["type"], "tool_result");
        assert_eq!(
            execute["receipt"]["result"]["output"],
            serde_json::json!({"answer":42})
        );

        let cancel = first_task
            .clone()
            .oneshot(command(
                "secret",
                "operation-cancel",
                Some(&attachment),
                serde_json::json!({
                    "type":"cancel",
                    "target_operation_id":"operation-execute"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(body(cancel).await["receipt"]["type"], "accepted");

        let teardown = first_task
            .oneshot(command(
                "secret",
                "operation-teardown",
                None,
                serde_json::json!({"type":"teardown"}),
            ))
            .await
            .unwrap();
        assert_eq!(body(teardown).await["receipt"]["type"], "accepted");
        assert_eq!(
            *driver.actions.lock().unwrap(),
            ["submit", "observe", "cancel", "release"]
        );
    }

    #[tokio::test]
    async fn authentication_and_operation_digest_conflicts_are_enforced() {
        let directory = tool_directory();
        let app = router(
            "secret",
            [(
                "aws-microvm".into(),
                Arc::new(FixtureDriver {
                    actions: StdMutex::new(Vec::new()),
                    refuse: false,
                }) as Arc<dyn Driver>,
            )],
            directory.path(),
        )
        .unwrap();
        assert_eq!(
            app.clone()
                .oneshot(command(
                    "wrong",
                    "operation-1",
                    None,
                    serde_json::json!({"type":"setup","configuration":{"driver":"aws-microvm"}}),
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let first = app
            .clone()
            .oneshot(command(
                "secret",
                "operation-1",
                None,
                serde_json::json!({"type":"setup","configuration":{"driver":"aws-microvm"}}),
            ))
            .await
            .unwrap();
        assert_eq!(body(first).await["receipt"]["type"], "accepted");
        let conflict = app
            .oneshot(command(
                "secret",
                "operation-1",
                None,
                serde_json::json!({"type":"setup","configuration":{"driver":"aws-microvm","idle_seconds":30}}),
            ))
            .await
            .unwrap();
        assert_eq!(body(conflict).await["receipt"]["type"], "conflict");
    }
}
