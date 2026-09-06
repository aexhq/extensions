use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use brain_protocol::{EnvironmentCommand, EnvironmentOperation, EnvironmentRequest};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex;

mod aws;

pub use aws::AwsDriver;

pub const MAX_ENVIRONMENT_REQUEST_BYTES: usize = 2 * 1024 * 1024;
const MAX_OPERATION_RECORDS: usize = 100_000;
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
        let remaining = deadline_at_ms.saturating_sub(now_ms());
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ToolManifest {
    name: String,
    description: String,
    input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_schema: Option<Value>,
    needs: Vec<String>,
    implementation: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OfficialToolImplementation {
    #[serde(rename = "type")]
    kind: String,
    version: u64,
    name: String,
}

#[derive(Clone)]
struct DriverState {
    bearer_digest: [u8; 32],
    drivers: Arc<HashMap<String, Arc<dyn Driver>>>,
    tools: Arc<HashMap<String, ToolBundle>>,
    environments: Arc<Mutex<HashMap<String, ActiveEnvironment>>>,
    operations: Arc<Mutex<OperationBook>>,
}

#[derive(Clone)]
struct ToolBundle {
    needs: Vec<String>,
    contract_digest: String,
    bundle_digest: String,
    bytes: Arc<[u8]>,
}

#[derive(Clone)]
struct ActiveEnvironment {
    driver: String,
    provider_configuration: Value,
    configuration_digest: String,
    needs: Vec<String>,
}

#[derive(Default)]
struct OperationBook {
    by_id: HashMap<String, OperationRecord>,
    order: VecDeque<String>,
}

struct OperationRecord {
    request_digest: String,
    receipt: Option<Value>,
}

#[derive(Deserialize)]
struct ToolRegistryEntry {
    contract_digest: String,
    filename: String,
    manifest: ToolManifest,
}

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
            valid_identifier(&name),
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
        environments: Arc::new(Mutex::new(HashMap::new())),
        operations: Arc::new(Mutex::new(OperationBook::default())),
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
        anyhow::ensure!(
            entry.manifest.name == name
                && canonical_digest(&entry.manifest)
                    .map_err(|error| anyhow::anyhow!(error.message))?
                    == entry.contract_digest.to_ascii_lowercase(),
            "Tool {name} registry manifest is invalid"
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
            needs: entry.manifest.needs,
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
        return http_failure(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let command: EnvironmentCommand = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return http_failure(StatusCode::BAD_REQUEST, "invalid Environment command"),
    };
    if command.contract != ENVIRONMENT_CONTRACT || !valid_operation(&command.operation) {
        return http_failure(StatusCode::BAD_REQUEST, "invalid Environment command");
    }
    let sequence = command.operation.sequence;
    let operation_key = operation_key(&command.operation);
    let request_digest = match canonical_digest(&command.operation) {
        Ok(value) => value,
        Err(error) => {
            return environment_response(
                sequence,
                failure_receipt("invalid_request", &error.message, false),
            );
        }
    };
    if let Some(receipt) = previous_operation(&state, &operation_key, &request_digest).await {
        return environment_response(sequence, receipt);
    }
    let receipt = match handle_operation(&state, &command.operation).await {
        Ok(receipt) => receipt,
        Err(error) => {
            tracing::warn!(
                sequence,
                status = %error.status,
                reason = %error.message,
                "Environment operation refused"
            );
            failure_receipt(
                if error.status.is_server_error() {
                    "unavailable"
                } else {
                    "invalid_request"
                },
                &error.message,
                error.status.is_server_error(),
            )
        }
    };
    complete_operation(&state, &operation_key, &request_digest, &receipt).await;
    environment_response(sequence, receipt)
}

async fn previous_operation(
    state: &DriverState,
    operation_key: &str,
    request_digest: &str,
) -> Option<Value> {
    let mut operations = state.operations.lock().await;
    if let Some(record) = operations.by_id.get(operation_key) {
        return Some(if record.request_digest == request_digest {
            record.receipt.clone().unwrap_or_else(|| {
                failure_receipt(
                    "unavailable",
                    "Environment operation with this sequence is still running",
                    true,
                )
            })
        } else {
            failure_receipt(
                "conflict",
                "Environment operation sequence was reused for a different request",
                false,
            )
        });
    }
    if operations.by_id.len() >= MAX_OPERATION_RECORDS
        && let Some(expired) = operations.order.pop_front()
    {
        operations.by_id.remove(&expired);
    }
    operations.by_id.insert(
        operation_key.to_owned(),
        OperationRecord {
            request_digest: request_digest.to_owned(),
            receipt: None,
        },
    );
    operations.order.push_back(operation_key.to_owned());
    None
}

async fn complete_operation(
    state: &DriverState,
    operation_key: &str,
    request_digest: &str,
    receipt: &Value,
) {
    let mut operations = state.operations.lock().await;
    if let Some(record) = operations.by_id.get_mut(operation_key)
        && record.request_digest == request_digest
    {
        record.receipt = Some(receipt.clone());
    }
}

async fn handle_operation(
    state: &DriverState,
    operation: &EnvironmentOperation,
) -> Result<Value, DriverError> {
    match &operation.request {
        EnvironmentRequest::Setup {
            configuration,
            needs,
        } => setup(state, operation, configuration, needs).await,
        EnvironmentRequest::Execute {
            implementation,
            needs,
            input,
            deadline_ms,
            ..
        } => execute(state, operation, implementation, needs, input, *deadline_ms).await,
        EnvironmentRequest::Cancel { target_sequence } => {
            cancel(state, operation, *target_sequence).await
        }
        EnvironmentRequest::Detach => {
            environment(state, operation).await?;
            Ok(accepted_receipt())
        }
        EnvironmentRequest::Teardown => teardown(state, operation).await,
        EnvironmentRequest::Call { name, .. } => Err(DriverError::invalid(format!(
            "unsupported AWS MicroVM Environment method {name}"
        ))),
    }
}

async fn setup(
    state: &DriverState,
    operation: &EnvironmentOperation,
    configuration: &Value,
    needs: &[String],
) -> Result<Value, DriverError> {
    needs_policy(needs)?;
    let mut provider_configuration = configuration.clone();
    let object = provider_configuration
        .as_object_mut()
        .ok_or_else(|| DriverError::invalid("Environment configuration must be an object"))?;
    let driver = object
        .remove("driver")
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| DriverError::invalid("Environment configuration has no driver"))?;
    if !state.drivers.contains_key(&driver) {
        return Err(DriverError::invalid("Environment driver is not configured"));
    }
    if object.keys().any(|key| key != "region") {
        return Err(DriverError::invalid(
            "unsupported provider configuration; lifecycle policy belongs to the caller",
        ));
    }
    let configuration_digest = canonical_digest(&(configuration, needs))?;
    let key = environment_key(operation);
    let mut environments = state.environments.lock().await;
    if let Some(existing) = environments.get(&key) {
        if existing.configuration_digest != configuration_digest {
            return Err(DriverError::invalid(
                "Environment is already configured differently",
            ));
        }
        return Ok(accepted_receipt());
    }
    environments.insert(
        key,
        ActiveEnvironment {
            driver,
            provider_configuration,
            configuration_digest,
            needs: needs.to_vec(),
        },
    );
    Ok(accepted_receipt())
}

async fn execute(
    state: &DriverState,
    operation: &EnvironmentOperation,
    implementation: &Value,
    needs: &[String],
    input: &Value,
    deadline_ms: u64,
) -> Result<Value, DriverError> {
    if deadline_ms == 0 {
        return Err(DriverError::invalid("execution deadline must be positive"));
    }
    let descriptor: OfficialToolImplementation = serde_json::from_value(implementation.clone())
        .map_err(|_| {
            DriverError::invalid("AWS MicroVM requires an official Tool implementation")
        })?;
    if descriptor.kind != "aex_official_tool"
        || descriptor.version != 1
        || !valid_identifier(&descriptor.name)
    {
        return Err(DriverError::invalid("unsupported Tool implementation"));
    }
    let tool = &descriptor.name;
    let bundle = state
        .tools
        .get(tool)
        .ok_or_else(|| DriverError::invalid("official Tool runtime is not installed"))?;
    let environment = environment(state, operation).await?;
    if !bundle.needs.iter().all(|need| needs.contains(need))
        || !needs.iter().all(|need| environment.needs.contains(need))
    {
        return Err(DriverError::invalid(
            "execution needs do not match setup grants and the installed Tool",
        ));
    }
    let deadline_at_ms = now_ms().saturating_add(deadline_ms);
    let driver = state
        .drivers
        .get(&environment.driver)
        .cloned()
        .ok_or_else(|| DriverError::unavailable("Environment driver is no longer configured"))?;
    let operation_id = provider_operation_id(operation.session_id.as_str(), operation.sequence);
    let request_digest = canonical_digest(operation)?;
    let policy = needs_policy(needs)?;
    let mut binding_value = provider_binding(&environment, operation);
    binding_value["policy"] = policy;
    let submit = match driver
        .dispatch(DispatchRequest {
            operation_id: operation_id.clone(),
            action: "submit".into(),
            request: serde_json::json!({
                "binding":binding_value,
                "operation":{
                    "operation_id":operation_id,
                    "request_digest":request_digest,
                    "kind":"invoke",
                    "descriptor_json":serde_json::to_string(&serde_json::json!({
                        "runtime":"node22",
                        "tool_name":tool,
                        "contract_digest":bundle.contract_digest,
                        "bundle_digest":bundle.bundle_digest
                    })).map_err(|_| DriverError::unavailable("Tool descriptor could not be encoded"))?,
                    "bundle_base64":base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        bundle.bytes.as_ref()
                    ),
                    "input_json":serde_json::to_string(&serde_json::json!({
                        "input":input,
                        "options":{}
                    })).map_err(|_| DriverError::invalid("Tool input cannot be encoded"))?,
                    "deadline_at_ms":deadline_at_ms.to_string()
                }
            }),
            deadline_at_ms: deadline_at_ms.to_string(),
        })
        .await {
            Ok(submit) => submit,
            Err(error) if error.status == StatusCode::BAD_REQUEST => return Err(error),
            Err(_) => return Ok(unknown_receipt("Environment did not confirm Tool submission")),
        };
    let Some(provider_reference) = submit.get("provider_operation_id").and_then(Value::as_str)
    else {
        return Ok(unknown_receipt(
            "Environment returned no operation reference",
        ));
    };
    let mut cursor: Option<String> = None;
    loop {
        if now_ms() >= deadline_at_ms {
            return Ok(serde_json::json!({
                "type":"unknown",
                "message":"Tool execution did not become terminal before its deadline"
            }));
        }
        let observation = match driver
            .dispatch(DispatchRequest {
                operation_id: provider_operation_id(
                    operation.session_id.as_str(),
                    operation.sequence,
                ),
                action: "observe".into(),
                request: serde_json::json!({
                    "binding":binding_value,
                    "provider_operation_id":provider_reference,
                    "cursor":cursor
                }),
                deadline_at_ms: deadline_at_ms.to_string(),
            })
            .await
        {
            Ok(observation) => observation,
            Err(_) => {
                return Ok(unknown_receipt(
                    "Environment lost contact with the Tool execution",
                ));
            }
        };
        cursor = observation
            .get("cursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        match observation.get("state").and_then(Value::as_str) {
            Some("pending" | "running") => continue,
            Some("completed") => {
                return Ok(serde_json::json!({
                    "type":"result",
                    "output":match terminal_value(&observation) { Ok(value) => value, Err(_) => return Ok(unknown_receipt("Environment returned an unreadable Tool result")) }
                }));
            }
            Some("failed") => {
                let terminal = match terminal_value(&observation) {
                    Ok(value) => value,
                    Err(_) => {
                        return Ok(unknown_receipt(
                            "Environment returned an unreadable Tool result",
                        ));
                    }
                };
                return Ok(failure_receipt(
                    "tool_failed",
                    &terminal_message(&terminal),
                    false,
                ));
            }
            Some("cancelled") => {
                return Ok(failure_receipt(
                    "cancelled",
                    "execution was cancelled",
                    false,
                ));
            }
            Some("deadline_exceeded") => {
                return Ok(failure_receipt(
                    "timeout",
                    "execution deadline elapsed",
                    false,
                ));
            }
            Some("interrupted" | "unknown") => {
                return Ok(serde_json::json!({
                    "type":"unknown",
                    "message":"Environment lost the Tool execution result"
                }));
            }
            _ => {
                return Ok(serde_json::json!({
                    "type":"unknown",
                    "message":"Environment returned an unknown Tool operation state"
                }));
            }
        }
    }
}

fn unknown_receipt(message: &str) -> Value {
    serde_json::json!({"type":"unknown", "message":message})
}

async fn cancel(
    state: &DriverState,
    operation: &EnvironmentOperation,
    target_sequence: u64,
) -> Result<Value, DriverError> {
    if target_sequence == 0 {
        return Err(DriverError::invalid("invalid cancellation target"));
    }
    let environment = environment(state, operation).await?;
    let driver = state
        .drivers
        .get(&environment.driver)
        .cloned()
        .ok_or_else(|| DriverError::unavailable("Environment driver is no longer configured"))?;
    driver
        .dispatch(DispatchRequest {
            operation_id: provider_operation_id(operation.session_id.as_str(), operation.sequence),
            action: "cancel".into(),
            request: serde_json::json!({
                "binding":provider_binding(&environment, operation),
                "target_operation_id":provider_operation_id(operation.session_id.as_str(), target_sequence)
            }),
            deadline_at_ms: now_ms().saturating_add(5_000).to_string(),
        })
        .await?;
    Ok(serde_json::json!({"type":"accepted"}))
}

async fn teardown(
    state: &DriverState,
    operation: &EnvironmentOperation,
) -> Result<Value, DriverError> {
    let environment = environment(state, operation).await?;
    let driver = state
        .drivers
        .get(&environment.driver)
        .cloned()
        .ok_or_else(|| DriverError::unavailable("Environment driver is no longer configured"))?;
    driver
        .dispatch(DispatchRequest {
            operation_id: provider_operation_id(operation.session_id.as_str(), operation.sequence),
            action: "release".into(),
            request: serde_json::json!({
                "binding":provider_binding(&environment, operation)
            }),
            deadline_at_ms: now_ms().saturating_add(30_000).to_string(),
        })
        .await?;
    state
        .environments
        .lock()
        .await
        .remove(&environment_key(operation));
    Ok(serde_json::json!({"type":"accepted"}))
}

async fn environment(
    state: &DriverState,
    operation: &EnvironmentOperation,
) -> Result<ActiveEnvironment, DriverError> {
    state
        .environments
        .lock()
        .await
        .get(&environment_key(operation))
        .cloned()
        .ok_or_else(|| DriverError::unavailable("Environment has not been set up"))
}

fn accepted_receipt() -> Value {
    serde_json::json!({"type":"accepted"})
}

// A provider root owns its VM and release operation, so sibling Environments need separate roots.
fn environment_key(operation: &EnvironmentOperation) -> String {
    format!(
        "env_{}",
        hex::encode(Sha256::digest(format!(
            "{}\0{}",
            operation.session_id, operation.environment
        )))
    )
}

fn needs_policy(needs: &[String]) -> Result<Value, DriverError> {
    let mut destinations = Vec::new();
    for need in needs {
        if matches!(
            need.as_str(),
            "file:///workspace"
                | "file:///workspace?access=write"
                | "pkg:apt/bash"
                | "pkg:apt/ripgrep"
        ) {
            continue;
        }
        let uri = reqwest::Url::parse(need)
            .map_err(|_| DriverError::invalid(format!("unmet need {need}")))?;
        if uri.scheme() != "https"
            || uri.host_str().is_none()
            || !uri.username().is_empty()
            || uri.password().is_some()
            || !matches!(uri.path(), "" | "/")
            || uri.query().is_some()
            || uri.fragment().is_some()
        {
            return Err(DriverError::invalid(format!("unmet need {need}")));
        }
        destinations.push(serde_json::json!({"protocol":"tls", "host":uri.host_str().unwrap(), "ports":[uri.port_or_known_default().unwrap()]}));
    }
    Ok(if destinations.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::json!({"network":{"outbound":"allowlist","destinations":destinations}})
    })
}

fn provider_binding(environment: &ActiveEnvironment, operation: &EnvironmentOperation) -> Value {
    serde_json::json!({
        "driver":environment.driver,
        "configuration":environment.provider_configuration,
        "policy":needs_policy(&environment.needs).expect("setup validated needs"),
        "tenant_id":operation.session_id,
        "session_id":environment_key(operation),
        "root_id":environment_key(operation),
        "parent_id":Value::Null,
        "environment_id":operation.environment
    })
}

fn operation_key(operation: &EnvironmentOperation) -> String {
    format!("{}:{}", operation.session_id, operation.sequence)
}

fn provider_operation_id(session_id: &str, sequence: u64) -> String {
    let digest = hex::encode(Sha256::digest(
        format!("{session_id}\0{sequence}").as_bytes(),
    ));
    format!("operation_{}", &digest[..32])
}

fn terminal_value(observation: &Value) -> Result<Value, DriverError> {
    observation
        .get("terminal_json")
        .and_then(Value::as_str)
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| DriverError::unavailable("AWS terminal result is invalid"))?
        .ok_or_else(|| DriverError::unavailable("AWS terminal result is missing"))
}

fn terminal_message(value: &Value) -> String {
    let message = value
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string());
    message.chars().take(4096).collect()
}

fn canonical_digest(value: &impl Serialize) -> Result<String, DriverError> {
    let bytes = serde_jcs::to_vec(value)
        .map_err(|_| DriverError::invalid("value cannot be canonicalized"))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn valid_operation(operation: &EnvironmentOperation) -> bool {
    operation.sequence > 0
        && valid_identifier(operation.environment.as_str())
        && valid_identifier(operation.session_id.as_str())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
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

fn failure_receipt(code: &str, message: &str, retryable: bool) -> Value {
    serde_json::json!({
        "type":"failure",
        "code":code,
        "message":message.chars().take(4096).collect::<String>(),
        "retryable":retryable
    })
}

fn environment_response(sequence: u64, receipt: Value) -> Response {
    Json(serde_json::json!({
        "contract":ENVIRONMENT_CONTRACT,
        "sequence":sequence,
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

fn http_failure(status: StatusCode, message: &str) -> Response {
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
    }

    #[async_trait]
    impl Driver for FixtureDriver {
        async fn dispatch(&self, request: DispatchRequest) -> Result<Value, DriverError> {
            assert!(request.deadline_at_ms.parse::<u64>().unwrap() > now_ms());
            assert_eq!(
                request.request["binding"]["configuration"],
                serde_json::json!({})
            );
            self.actions.lock().unwrap().push(request.action.clone());
            match request.action.as_str() {
                "submit" => {
                    assert_eq!(request.request.as_object().unwrap().len(), 2);
                    assert_eq!(
                        request.request["binding"]["policy"],
                        needs_policy(&[]).unwrap()
                    );
                    Ok(serde_json::json!({"provider_operation_id":"provider-1"}))
                }
                "observe" => Ok(serde_json::json!({
                    "state":"completed",
                    "cursor":"1",
                    "chunks":[],
                    "terminal_json":"{\"answer\":42}"
                })),
                "cancel" | "release" => Ok(serde_json::json!({})),
                _ => Err(DriverError::invalid("unexpected fixture action")),
            }
        }
    }

    struct LostResultDriver {
        fail_submit: bool,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl Driver for LostResultDriver {
        async fn dispatch(&self, request: DispatchRequest) -> Result<Value, DriverError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if request.action == "submit" && !self.fail_submit {
                Ok(serde_json::json!({"provider_operation_id":"provider-1"}))
            } else {
                Err(DriverError::unavailable("connection lost"))
            }
        }
    }

    #[tokio::test]
    async fn lost_submission_or_result_remains_unknown_without_replay() {
        for fail_submit in [true, false] {
            let tool = manifest("echo");
            let directory = tool_directory(&tool);
            let driver = Arc::new(LostResultDriver {
                fail_submit,
                calls: std::sync::atomic::AtomicUsize::new(0),
            });
            let app = router(
                "secret",
                [("aws-microvm".into(), driver.clone() as Arc<dyn Driver>)],
                directory.path(),
            )
            .unwrap();
            let setup = app.clone().oneshot(command("secret", 1, serde_json::json!({"type":"setup","needs":[],"configuration":{"driver":"aws-microvm"}}))).await.unwrap();
            assert_eq!(body(setup).await["receipt"]["type"], "accepted");
            for _ in 0..2 {
                let response = app.clone().oneshot(command("secret", 2, serde_json::json!({"type":"execute", "implementation":tool.implementation, "needs":[], "input":{}, "deadline_ms":60_000}))).await.unwrap();
                assert_eq!(body(response).await["receipt"]["type"], "unknown");
            }
            assert_eq!(
                driver.calls.load(std::sync::atomic::Ordering::SeqCst),
                if fail_submit { 1 } else { 2 }
            );
        }
    }

    fn manifest(name: &str) -> ToolManifest {
        ToolManifest {
            name: name.into(),
            description: "Echo input.".into(),
            input_schema: serde_json::json!({"type":"object"}),
            output_schema: Some(serde_json::json!({"type":"object"})),
            needs: vec![],
            implementation: serde_json::json!({
                "type":"aex_official_tool",
                "version":1,
                "name":name
            }),
        }
    }

    fn tool_directory(tool: &ToolManifest) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("echo.mjs"), "export default {};").unwrap();
        std::fs::write(
            directory.path().join("registry.json"),
            serde_json::json!({
                "echo":{
                    "contract_digest":canonical_digest(tool).unwrap(),
                    "filename":"echo.mjs",
                    "manifest":tool
                }
            })
            .to_string(),
        )
        .unwrap();
        directory
    }

    fn command(token: &str, sequence: u64, request: Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/v1/operations")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({
                    "contract":"environment/v1",
                    "operation":{
                        "sequence":sequence,
                        "environment":"workspace",
                        "session_id":"session-1",
                        "request":request
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
    async fn lifecycle_resolves_an_official_tool_at_execution() {
        let tool = manifest("echo");
        let directory = tool_directory(&tool);
        let driver = Arc::new(FixtureDriver {
            actions: StdMutex::new(Vec::new()),
        });
        let app = router(
            "secret",
            [("aws-microvm".into(), driver.clone() as Arc<dyn Driver>)],
            directory.path(),
        )
        .unwrap();

        let setup = app
            .clone()
            .oneshot(command(
                "secret",
                1,
                serde_json::json!({"type":"setup","needs":["https://api.example.com"],"configuration":{"driver":"aws-microvm"}}),
            ))
            .await
            .unwrap();
        let setup = body(setup).await;
        assert_eq!(setup["sequence"], 1);
        assert_eq!(setup["receipt"]["type"], "accepted");

        let execute = app
            .clone()
            .oneshot(command(
                "secret",
                3,
                serde_json::json!({
                    "type":"execute",
                    "implementation":tool.implementation,
                    "needs":[],
                    "input":{"value":42},
                    "deadline_ms":60_000
                }),
            ))
            .await
            .unwrap();
        let execute = body(execute).await;
        assert_eq!(execute["receipt"]["type"], "result");
        assert_eq!(
            execute["receipt"]["output"],
            serde_json::json!({"answer":42})
        );

        let cancel = app
            .clone()
            .oneshot(command(
                "secret",
                4,
                serde_json::json!({"type":"cancel","target_sequence":3}),
            ))
            .await
            .unwrap();
        assert_eq!(body(cancel).await["receipt"]["type"], "accepted");

        let teardown = app
            .oneshot(command("secret", 5, serde_json::json!({"type":"teardown"})))
            .await
            .unwrap();
        assert_eq!(body(teardown).await["receipt"]["type"], "accepted");
        assert_eq!(
            *driver.actions.lock().unwrap(),
            ["submit", "observe", "cancel", "release"]
        );
    }

    #[tokio::test]
    async fn rejects_unknown_tool_implementations_and_reused_sequences() {
        let tool = manifest("echo");
        let directory = tool_directory(&tool);
        let app = router(
            "secret",
            [(
                "aws-microvm".into(),
                Arc::new(FixtureDriver {
                    actions: StdMutex::new(Vec::new()),
                }) as Arc<dyn Driver>,
            )],
            directory.path(),
        )
        .unwrap();
        assert_eq!(
            app.clone()
                .oneshot(command(
                    "wrong",
                    1,
                        serde_json::json!({"type":"setup","needs":[],"configuration":{"driver":"aws-microvm"}}),
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
                1,
                serde_json::json!({"type":"setup","needs":[],"configuration":{"driver":"aws-microvm"}}),
            ))
            .await
            .unwrap();
        assert_eq!(body(first).await["receipt"]["type"], "accepted");
        let conflict = app
            .clone()
            .oneshot(command(
                "secret",
                1,
                serde_json::json!({"type":"setup","needs":[],"configuration":{"driver":"aws-microvm","idle_seconds":30}}),
            ))
            .await
            .unwrap();
        assert_eq!(body(conflict).await["receipt"]["code"], "conflict");

        let mut wrong = tool;
        wrong.implementation = serde_json::json!({"type":"arbitrary","version":1,"name":"echo"});
        let attach = app
            .oneshot(command(
                "secret",
                2,
                serde_json::json!({
                    "type":"execute",
                    "implementation":wrong.implementation, "needs":[], "input":{}, "deadline_ms":1000
                }),
            ))
            .await
            .unwrap();
        assert_eq!(body(attach).await["receipt"]["type"], "failure");
    }
}
