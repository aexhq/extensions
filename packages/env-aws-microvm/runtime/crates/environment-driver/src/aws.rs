use std::sync::Arc;

use async_trait::async_trait;
use aws_microvm_controller::{
    AwsMicrovmEnvironment, PROVIDER_MAX_DURATION_SECONDS, PROVIDER_MAX_IDLE_SECONDS, TargetLifetime,
};
use aws_sdk_dynamodb::{Client as DynamoClient, types::AttributeValue};
use base64::Engine as _;
use environment_wire::{
    AcknowledgeTerminalRequest, CancelRequest, EnvironmentError, ObserveRequest, OperationEnvelope,
    OperationRef, OperationState, SealedBinding, TerminalOutcome,
};
use environment_wire::{EnvironmentPort as _, SessionPreparationPort as _};
use environment_wire::{canonical_digest, operation_request_digest};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::{DispatchRequest, Driver, DriverError};

const DRIVER: &str = "aws-microvm";
const MAX_OBSERVE_WAIT_MS: u64 = 1_000;

pub struct AwsDriver {
    environment: Arc<AwsMicrovmEnvironment>,
    maximum_lifetime: TargetLifetime,
    operation_references: OperationReferenceStore,
}

const MAX_IDLE_SECONDS_ENV: &str = "ENVIRONMENT_MAX_SESSION_IDLE_SECONDS";
const MAX_DURATION_SECONDS_ENV: &str = "ENVIRONMENT_MAX_SESSION_DURATION_SECONDS";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    driver: String,
    configuration: EnvironmentConfiguration,
    policy: Value,
    tenant_id: String,
    session_id: String,
    root_id: String,
    parent_id: Option<String>,
    environment_id: String,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct EnvironmentConfiguration {
    region: Option<String>,
    idle_seconds: Option<u64>,
    maximum_seconds: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitBody {
    binding: Binding,
    operation: ComponentOperation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ComponentOperation {
    operation_id: String,
    request_digest: String,
    kind: String,
    descriptor_json: String,
    bundle_base64: String,
    input_json: String,
    deadline_at_ms: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolDescriptor {
    runtime: String,
    tool_name: String,
    contract_digest: String,
    bundle_digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationBody {
    binding: Binding,
    provider_operation_id: String,
    cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelBody {
    binding: Binding,
    target_operation_id: String,
}

struct OperationReferenceStore {
    client: DynamoClient,
    table: String,
}

struct StoredOperationReference {
    request_digest: String,
    provider_operation_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AcknowledgeBody {
    binding: Binding,
    provider_operation_id: String,
    terminal: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseBody {
    binding: Binding,
}

impl AwsDriver {
    pub async fn from_env() -> anyhow::Result<Self> {
        let maximum_lifetime = TargetLifetime::new(
            optional_seconds(MAX_IDLE_SECONDS_ENV, PROVIDER_MAX_IDLE_SECONDS)?,
            optional_seconds(MAX_DURATION_SECONDS_ENV, PROVIDER_MAX_DURATION_SECONDS)?,
        )
        .map_err(anyhow::Error::msg)?;
        let sdk = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let table = std::env::var("ENVIRONMENT_REGISTRY_TABLE")?;
        anyhow::ensure!(
            !table.trim().is_empty(),
            "ENVIRONMENT_REGISTRY_TABLE is empty"
        );
        Ok(Self {
            environment: AwsMicrovmEnvironment::from_env().await?,
            maximum_lifetime,
            operation_references: OperationReferenceStore {
                client: DynamoClient::new(&sdk),
                table,
            },
        })
    }

    fn parse<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, DriverError> {
        serde_json::from_value(value)
            .map_err(|_| DriverError::invalid("invalid AWS Environment request"))
    }

    fn check_binding(binding: &Binding) -> Result<(), DriverError> {
        if binding.driver != DRIVER
            || binding.tenant_id.is_empty()
            || binding.session_id.is_empty()
            || binding.root_id.is_empty()
            || binding.environment_id.is_empty()
        {
            return Err(DriverError::invalid("invalid AWS Environment binding"));
        }
        if binding
            .configuration
            .region
            .as_ref()
            .is_some_and(|region| region.trim().is_empty())
        {
            return Err(DriverError::invalid("invalid AWS Environment region"));
        }
        let _ = &binding.parent_id;
        Ok(())
    }

    fn target_lifetime(&self, binding: &Binding) -> Result<TargetLifetime, DriverError> {
        bounded_target_lifetime(&binding.configuration, self.maximum_lifetime)
    }

    fn operation_ref(encoded: &str) -> Result<OperationRef, DriverError> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| DriverError::invalid("invalid AWS operation reference"))?;
        serde_json::from_slice(&bytes)
            .map_err(|_| DriverError::invalid("invalid AWS operation reference"))
    }

    fn encode_operation(operation: &OperationRef) -> Result<String, DriverError> {
        let bytes = serde_json::to_vec(operation).map_err(|_| {
            DriverError::unavailable("AWS operation reference could not be encoded")
        })?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
    }

    async fn submit(&self, request: DispatchRequest) -> Result<Value, DriverError> {
        let body: SubmitBody = Self::parse(request.request)?;
        Self::check_binding(&body.binding)?;
        if body.operation.operation_id != request.operation_id || body.operation.kind != "invoke" {
            return Err(DriverError::invalid("invalid AWS Environment operation"));
        }
        if let Some(stored) = self
            .operation_references
            .get(&body.binding.root_id, &body.operation.operation_id)
            .await?
        {
            if stored.request_digest != body.operation.request_digest {
                return Err(DriverError::invalid(
                    "AWS Environment operation identity has a different request digest",
                ));
            }
            return Ok(json!({"provider_operation_id":stored.provider_operation_id}));
        }
        let descriptor: ToolDescriptor = serde_json::from_str(&body.operation.descriptor_json)
            .map_err(|_| DriverError::invalid("invalid Tool descriptor"))?;
        if descriptor.runtime != "node22" {
            return Err(DriverError::invalid(
                "AWS Environment supports only node22 Tool bundles",
            ));
        }
        let bundle = base64::engine::general_purpose::STANDARD
            .decode(&body.operation.bundle_base64)
            .map_err(|_| DriverError::invalid("invalid Tool bundle"))?;
        if bundle.is_empty() || hex_digest(&bundle) != descriptor.bundle_digest {
            return Err(DriverError::invalid(
                "Tool bundle does not match its descriptor",
            ));
        }
        let deadline_at_ms = body
            .operation
            .deadline_at_ms
            .parse::<u64>()
            .map_err(|_| DriverError::invalid("invalid Tool deadline"))?;
        let input: Value = serde_json::from_str(&body.operation.input_json)
            .map_err(|_| DriverError::invalid("invalid Tool input"))?;
        let lifetime = self.target_lifetime(&body.binding)?;
        let network = network(&body.binding.policy)?;
        let policy_digest = canonical_digest(&body.binding.policy)
            .map_err(|_| DriverError::invalid("Environment policy cannot be canonicalized"))?;
        let implementation_identity = hex_digest(
            format!(
                "{DRIVER}\0{}\0{}",
                descriptor.runtime, descriptor.bundle_digest
            )
            .as_bytes(),
        );
        let binding_id = format!(
            "component-{}",
            hex_digest(
                format!(
                    "{}\0{}\0{}",
                    body.binding.environment_id, descriptor.tool_name, descriptor.bundle_digest
                )
                .as_bytes()
            )
        );
        let object_id = format!("component-{}", descriptor.bundle_digest);
        let provider_configuration = managed_configuration(&body.binding.configuration);
        let binding_value = json!({
            "binding_id": binding_id,
            "bundle": {
                "bundle_digest": descriptor.bundle_digest,
                "bytes": bundle.len(),
                "contract_digest": descriptor.contract_digest,
                "environment_name": body.binding.environment_id,
                "execute_path": "/tool/runtime.mjs",
                "layers": [{
                    "digest": descriptor.bundle_digest,
                    "bytes": bundle.len(),
                    "media_type": "application/javascript+esm",
                    "mount_path": "/tool/runtime.mjs",
                    "unpack": "file",
                    "object": {
                        "bytes": bundle.len(),
                        "object_id": object_id,
                        "sha256": descriptor.bundle_digest
                    }
                }],
                "required_env": [],
                "target": "linux-arm64",
                "tool_name": descriptor.tool_name
            },
            "capability": descriptor.tool_name,
            "configuration": provider_configuration,
            "contract_digest": descriptor.contract_digest,
            "environment_name": body.binding.environment_id,
            "extension": "@aexhq/env-aws-microvm",
            "implementation_identity": implementation_identity,
            "policy_digest": policy_digest,
            "profile": {
                "kind": "computer",
                "platform": "linux-arm64",
                "network": "allowlist",
                "recovery": "retained"
            },
            "protocol": "environment/v1",
            "required_capabilities": ["execution"],
            "root_id": body.binding.root_id,
            "session_id": body.binding.session_id
        });
        let binding: SealedBinding = serde_json::from_value(binding_value)
            .map_err(|_| DriverError::invalid("Tool descriptor cannot form an AWS binding"))?;
        let binding_digest = canonical_digest(&binding)
            .map_err(|_| DriverError::invalid("AWS binding cannot be canonicalized"))?;
        let mut envelope: OperationEnvelope = serde_json::from_value(json!({
            "binding_ref": format!("binding:{}", binding_digest.as_str()),
            "caller_id": body.binding.session_id,
            "capability": binding.capability,
            "deadline_at_ms": deadline_at_ms,
            "fence": 1,
            "input": {"kind":"inline", "value": input},
            "network": network,
            "operation_id": body.operation.operation_id,
            "phase": "execute",
            "request_digest": "0".repeat(64),
            "resources": {
                "max_output_bytes": environment_wire::MAX_TOOL_TERMINAL_INLINE_BYTES,
                "timeout_ms": deadline_at_ms.saturating_sub(now_ms()).max(1)
            },
            "root_id": body.binding.root_id,
            "session_id": body.binding.session_id,
            "trace": {},
            "turn_id": request.operation_id
        }))
        .map_err(|_| DriverError::invalid("Tool operation cannot form an AWS request"))?;
        envelope.request_digest = operation_request_digest(&envelope);
        let wait = deadline_at_ms
            .saturating_sub(now_ms())
            .min(MAX_OBSERVE_WAIT_MS);
        let receipt = self
            .environment
            .submit_component(binding, envelope, bundle, wait, lifetime)
            .await
            .map_err(map_environment_error)?;
        let provider_operation_id = Self::encode_operation(&receipt.operation)?;
        self.operation_references
            .put(
                &body.binding.root_id,
                &body.operation.operation_id,
                &body.operation.request_digest,
                &provider_operation_id,
            )
            .await?;
        Ok(json!({"provider_operation_id":provider_operation_id}))
    }

    async fn observe(&self, request: DispatchRequest) -> Result<Value, DriverError> {
        let body: OperationBody = Self::parse(request.request)?;
        Self::check_binding(&body.binding)?;
        let operation = Self::operation_ref(&body.provider_operation_id)?;
        let observation = self
            .environment
            .observe(ObserveRequest {
                cursor: body
                    .cursor
                    .unwrap_or_default()
                    .parse()
                    .map_err(|_| DriverError::invalid("invalid AWS operation cursor"))?,
                operation,
                wait_ms: MAX_OBSERVE_WAIT_MS,
            })
            .await
            .map_err(map_environment_error)?;
        observation_value(observation)
    }

    async fn cancel(&self, request: DispatchRequest) -> Result<Value, DriverError> {
        let body: CancelBody = Self::parse(request.request)?;
        Self::check_binding(&body.binding)?;
        let stored = self
            .operation_references
            .get(&body.binding.root_id, &body.target_operation_id)
            .await?
            .ok_or_else(|| {
                DriverError::unavailable("AWS Environment operation reference is unavailable")
            })?;
        self.environment
            .cancel(CancelRequest {
                operation: Self::operation_ref(&stored.provider_operation_id)?,
                reason: "component Environment cancellation"
                    .parse()
                    .expect("valid reason"),
            })
            .await
            .map_err(map_environment_error)?;
        Ok(json!({}))
    }

    async fn acknowledge(&self, request: DispatchRequest) -> Result<Value, DriverError> {
        let body: AcknowledgeBody = Self::parse(request.request)?;
        Self::check_binding(&body.binding)?;
        let operation = Self::operation_ref(&body.provider_operation_id)?;
        let observation = self
            .environment
            .observe(ObserveRequest {
                cursor: "".parse().expect("valid cursor"),
                operation: operation.clone(),
                wait_ms: 0,
            })
            .await
            .map_err(map_environment_error)?;
        let terminal = observation
            .terminal
            .ok_or_else(|| DriverError::invalid("AWS operation is not terminal"))?;
        if terminal.inline.as_ref() != Some(&body.terminal) {
            return Err(DriverError::invalid(
                "terminal acknowledgement does not match AWS result",
            ));
        }
        self.environment
            .acknowledge_terminal(AcknowledgeTerminalRequest {
                operation,
                terminal_digest: terminal.terminal_digest,
            })
            .await
            .map_err(map_environment_error)?;
        Ok(json!({}))
    }

    async fn release(&self, request: DispatchRequest) -> Result<Value, DriverError> {
        let body: ReleaseBody = Self::parse(request.request)?;
        Self::check_binding(&body.binding)?;
        self.environment
            .purge_tree(&body.binding.root_id)
            .await
            .map_err(map_environment_error)?;
        Ok(json!({}))
    }
}

impl OperationReferenceStore {
    async fn get(
        &self,
        root_id: &str,
        operation_id: &str,
    ) -> Result<Option<StoredOperationReference>, DriverError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("root_id", AttributeValue::S(root_id.to_owned()))
            .key(
                "target_key",
                AttributeValue::S(format!("operation#{operation_id}")),
            )
            .consistent_read(true)
            .send()
            .await
            .map_err(|_| {
                DriverError::unavailable("Environment operation registry is unavailable")
            })?;
        let Some(item) = output.item else {
            return Ok(None);
        };
        let request_digest = item
            .get("request_digest")
            .and_then(|value| value.as_s().ok())
            .cloned()
            .ok_or_else(|| DriverError::unavailable("Environment operation registry is invalid"))?;
        let provider_operation_id = item
            .get("provider_operation_id")
            .and_then(|value| value.as_s().ok())
            .cloned()
            .ok_or_else(|| DriverError::unavailable("Environment operation registry is invalid"))?;
        Ok(Some(StoredOperationReference {
            request_digest,
            provider_operation_id,
        }))
    }

    async fn put(
        &self,
        root_id: &str,
        operation_id: &str,
        request_digest: &str,
        provider_operation_id: &str,
    ) -> Result<(), DriverError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item("root_id", AttributeValue::S(root_id.to_owned()))
            .item(
                "target_key",
                AttributeValue::S(format!("operation#{operation_id}")),
            )
            .item(
                "request_digest",
                AttributeValue::S(request_digest.to_owned()),
            )
            .item(
                "provider_operation_id",
                AttributeValue::S(provider_operation_id.to_owned()),
            )
            .send()
            .await
            .map_err(|_| {
                DriverError::unavailable("Environment operation registry is unavailable")
            })?;
        Ok(())
    }
}

fn optional_seconds(name: &str, default: u64) -> anyhow::Result<u64> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(default);
    };
    let value = value
        .into_string()
        .map_err(|_| anyhow::anyhow!("{name} must contain UTF-8 decimal text"))?;
    value
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("{name} must contain an unsigned decimal integer"))
}

fn bounded_target_lifetime(
    configuration: &EnvironmentConfiguration,
    maximum: TargetLifetime,
) -> Result<TargetLifetime, DriverError> {
    let maximum_seconds = configuration
        .maximum_seconds
        .unwrap_or(maximum.maximum_seconds);
    let requested = TargetLifetime::new(
        configuration
            .idle_seconds
            .unwrap_or(maximum.idle_seconds.min(maximum_seconds)),
        maximum_seconds,
    )
    .map_err(DriverError::invalid)?;
    if requested.idle_seconds > maximum.idle_seconds
        || requested.maximum_seconds > maximum.maximum_seconds
    {
        return Err(DriverError::invalid(
            "AWS target lifetime exceeds the deployment maximum",
        ));
    }
    Ok(requested)
}

#[async_trait]
impl Driver for AwsDriver {
    async fn dispatch(&self, request: DispatchRequest) -> Result<Value, DriverError> {
        match request.action.as_str() {
            "submit" => self.submit(request).await,
            "observe" => self.observe(request).await,
            "cancel" => self.cancel(request).await,
            "acknowledge" => self.acknowledge(request).await,
            "release" => self.release(request).await,
            _ => Err(DriverError::invalid("unsupported AWS Environment action")),
        }
    }
}

fn observation_value(
    observation: environment_wire::OperationObservation,
) -> Result<Value, DriverError> {
    let state = match observation.state {
        OperationState::Accepted => "pending",
        OperationState::Running => "running",
        OperationState::Unknown => "unknown",
        OperationState::Terminal => match observation
            .terminal
            .as_ref()
            .map(|terminal| terminal.outcome)
        {
            Some(TerminalOutcome::Completed) => "completed",
            Some(TerminalOutcome::Cancelled) => "cancelled",
            Some(TerminalOutcome::DeadlineExceeded) => "deadline_exceeded",
            Some(TerminalOutcome::Failed) => "failed",
            Some(TerminalOutcome::Interrupted) => "interrupted",
            None => {
                return Err(DriverError::unavailable(
                    "AWS terminal observation has no result",
                ));
            }
        },
    };
    let terminal_json = observation
        .terminal
        .and_then(|terminal| terminal.inline)
        .map(|value| serde_json::to_string(&value))
        .transpose()
        .map_err(|_| DriverError::unavailable("AWS terminal result could not be encoded"))?;
    Ok(json!({
        "state": state,
        "cursor": observation.next_cursor,
        "chunks": observation.output,
        "terminal_json": terminal_json
    }))
}

fn network(policy: &Value) -> Result<Value, DriverError> {
    let network = policy.get("network").cloned().unwrap_or_else(|| json!({}));
    let outbound = network
        .get("outbound")
        .and_then(Value::as_str)
        .unwrap_or("none");
    match outbound {
        "none" => Ok(json!({"kind":"none"})),
        "public" => Ok(json!({"kind":"public"})),
        "allowlist" => Ok(json!({
            "kind":"allowlist",
            "destinations": network.get("destinations").cloned().unwrap_or_else(|| json!([]))
        })),
        _ => Err(DriverError::invalid(
            "unsupported AWS Environment network policy",
        )),
    }
}

fn managed_configuration(configuration: &EnvironmentConfiguration) -> Value {
    configuration
        .region
        .as_ref()
        .map_or_else(|| json!({}), |region| json!({"region": region}))
}

fn map_environment_error(error: EnvironmentError) -> DriverError {
    if error.retryable {
        DriverError::unavailable("AWS Environment is temporarily unavailable")
    } else {
        DriverError::invalid("AWS Environment rejected the request")
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_reference_round_trips_without_process_state() {
        let operation: OperationRef = serde_json::from_value(json!({
            "generation": "generation-1",
            "operation_id": "operation-1",
            "receipt_ref": "receipt-1",
            "request_digest": "a".repeat(64),
            "target": {
                "binding_ref": "binding-1",
                "kind": "environment",
                "root_id": "root-1",
                "session_id": "session-1"
            },
            "target_ref": "target-1"
        }))
        .unwrap();
        let encoded = AwsDriver::encode_operation(&operation).unwrap();
        assert_eq!(
            serde_json::to_value(AwsDriver::operation_ref(&encoded).unwrap()).unwrap(),
            serde_json::to_value(operation).unwrap()
        );
    }

    #[test]
    fn component_network_policy_maps_only_supported_modes() {
        assert_eq!(network(&json!({})).unwrap(), json!({"kind":"none"}));
        assert_eq!(
            network(&json!({"network":{"outbound":"public"}})).unwrap(),
            json!({"kind":"public"})
        );
        assert!(network(&json!({"network":{"outbound":"private"}})).is_err());
    }

    #[test]
    fn managed_configuration_contains_only_present_provider_options() {
        assert_eq!(
            managed_configuration(&EnvironmentConfiguration {
                idle_seconds: Some(30),
                maximum_seconds: Some(60),
                ..EnvironmentConfiguration::default()
            }),
            json!({})
        );
        assert_eq!(
            managed_configuration(&EnvironmentConfiguration {
                region: Some("us-east-1".into()),
                idle_seconds: Some(30),
                maximum_seconds: Some(60)
            }),
            json!({"region":"us-east-1"})
        );
    }

    #[test]
    fn component_lifetime_is_finite_and_capped_by_the_driver() {
        let maximum = TargetLifetime::new(120, 3_600).unwrap();
        let defaulted =
            bounded_target_lifetime(&EnvironmentConfiguration::default(), maximum).unwrap();
        assert_eq!(defaulted, maximum);
        let requested = bounded_target_lifetime(
            &EnvironmentConfiguration {
                idle_seconds: Some(30),
                maximum_seconds: Some(600),
                ..EnvironmentConfiguration::default()
            },
            maximum,
        )
        .unwrap();
        assert_eq!(requested, TargetLifetime::new(30, 600).unwrap());
        assert!(
            bounded_target_lifetime(
                &EnvironmentConfiguration {
                    idle_seconds: Some(121),
                    ..EnvironmentConfiguration::default()
                },
                maximum,
            )
            .is_err()
        );
    }
}
