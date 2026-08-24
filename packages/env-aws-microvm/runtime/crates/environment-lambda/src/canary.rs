//! Explicit live-image proofs for provider behavior and the direct-public connector boundary.
//!
//! Every canary follows one arc: seal random identities into a `RunPayload`, launch the exact
//! image version, run its proof against the known target, and always terminate the target. The
//! proofs differ only in their sealed network ceiling and their in-guest script.

use std::{
    net::Ipv4Addr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, bail, ensure};
use aws_sdk_lambdamicrovms::types::MicrovmState;
use brain_protocol::contract::{ENVIRONMENT_CONTRACT_DIGEST, sandbox_execution_request_digest};
use brain_protocol::environment::{
    AcknowledgeTerminalRequest, NetworkCeiling, ObserveRequest, OperationRef,
    SandboxExecutionRequest, TerminalOutcome, TerminalResult,
};
use environment_core::connector::{
    ConnectorCatalog, ConnectorClass, ConnectorRef, GatewayAuthority,
};
use environment_core::materialization::ControlToken;
use environment_wire::{
    AllowlistProxy, RequestCall, RequestFrame, ResponseFrame, ResponseReply, RunPayload,
};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message;

use crate::control::{AUTH_HEADER, Control, ControlError, Microvm, is_gone, is_terminated};
use crate::launch::{self, GuestConnectError, LaunchedEnvironment};

const CANARY_LIFETIME_MS: u64 = 10 * 60 * 1_000;
const STATE_TIMEOUT: Duration = Duration::from_secs(120);
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const RUN_HOOK_STABILITY_WINDOW: Duration = Duration::from_secs(120);
const REQUIRED_CONSECUTIVE_502: usize = 3;

pub struct NoRespawnCanaryConfig {
    pub image_arn: String,
    pub image_version: String,
    pub none_connector: ConnectorRef,
}

pub struct NetworkBoundaryCanaryConfig {
    pub image_arn: String,
    pub image_version: String,
    pub connectors: ConnectorCatalog,
    pub gateway_authority: GatewayAuthority,
    pub customer_environment_hosts: [String; 2],
}

/// The two connector classes the restricted-network proof accepts. A separate type instead of
/// `ConnectorClass` so the public variant is unrepresentable rather than runtime-rejected.
#[derive(Clone, Copy)]
enum RestrictedClass {
    None,
    Allowlist,
}

impl RestrictedClass {
    fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Allowlist => "allowlist",
        }
    }

    fn class(self) -> ConnectorClass {
        match self {
            Self::None => ConnectorClass::None,
            Self::Allowlist => ConnectorClass::Allowlist,
        }
    }
}

/// Exercises every deployed connector class against the same exact image version. Restricted
/// connector targets run and terminate sequentially so the dev plane's one-GiB admission budget
/// is never exceeded.
pub async fn run_network_boundary_canary(
    control: &Control,
    cfg: NetworkBoundaryCanaryConfig,
) -> anyhow::Result<()> {
    validate_customer_environment_hosts(&cfg.customer_environment_hosts, control.region())?;
    for class in [RestrictedClass::None, RestrictedClass::Allowlist] {
        run_restricted_network_canary(
            control,
            &cfg.image_arn,
            &cfg.image_version,
            cfg.connectors.resolve(class.class()),
            &cfg.gateway_authority,
            class,
        )
        .await?;
    }
    run_public_network_canary(control, &cfg).await
}

struct KnownTargetSeal {
    target_id: String,
    generation: String,
    root_id: String,
    session_id: String,
    operation_id: String,
    control_token: ControlToken,
}

/// Seals one canary run: random single-use identities plus the exact run payload. The
/// `target_id` is filled in by [`with_canary_target`] after the provider launch.
fn seal_canary_run(
    label: &str,
    connector: ConnectorClass,
    network: NetworkCeiling,
    allowlist_proxy: Option<AllowlistProxy>,
    canary_exit: bool,
) -> anyhow::Result<(KnownTargetSeal, RunPayload)> {
    let nonce = hex::encode(rand::random::<[u8; 12]>());
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock precedes Unix epoch")?
        .as_millis() as u64;
    let seal = KnownTargetSeal {
        target_id: String::new(),
        generation: format!("{label}-canary-generation-{nonce}"),
        root_id: format!("{label}-canary-root-{nonce}"),
        session_id: format!("{label}-canary-session-{nonce}"),
        operation_id: format!("{label}-canary-{nonce}"),
        control_token: canary_control_token(),
    };
    let payload = RunPayload {
        contract_digest: ENVIRONMENT_CONTRACT_DIGEST.trim().into(),
        generation: seal.generation.clone(),
        expires_at_ms: now_ms.saturating_add(CANARY_LIFETIME_MS),
        root_id: seal.root_id.clone(),
        owner_session_id: seal.session_id.clone(),
        connector,
        resource_class: "microvm-1gb".into(),
        resources: serde_json::from_value(serde_json::json!({
            "max_output_bytes": 4_096,
            "timeout_ms": 30_000
        }))?,
        network,
        control_token: seal.control_token.clone(),
        allowlist_proxy,
        canary_exit_after_operation_id: canary_exit.then(|| seal.operation_id.clone()),
    };
    Ok((seal, payload))
}

/// One canary's exact launch identity: the immutable image version plus the connector class ref
/// it must run behind.
struct CanaryLaunch<'a> {
    image_arn: &'a str,
    image_version: &'a str,
    connector: &'a ConnectorRef,
}

/// Launches the sealed target, runs the proof body against it, and always terminates the target,
/// merging proof and cleanup outcomes so neither failure can mask the other.
async fn with_canary_target<Fut>(
    control: &Control,
    launch: CanaryLaunch<'_>,
    label: &str,
    seal: KnownTargetSeal,
    payload: &RunPayload,
    body: impl FnOnce(KnownTargetSeal) -> Fut,
) -> anyhow::Result<()>
where
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let vm = launch_canary_target(
        control,
        launch.image_arn,
        launch.image_version,
        &serde_json::to_string(payload)?,
        &format!("environments-{}", seal.operation_id),
        launch.connector,
    )
    .await
    .with_context(|| format!("launching the {label} canary"))?;
    let seal = KnownTargetSeal {
        target_id: vm.id,
        ..seal
    };
    let target_id = seal.target_id.clone();
    let result = body(seal).await;
    let cleanup = terminate_known_target(control, &target_id).await;
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(canary), Ok(())) => Err(canary),
        (Ok(()), Err(cleanup)) => {
            Err(cleanup.context(format!("{label} canary passed but target cleanup failed")))
        }
        (Err(canary), Err(cleanup)) => {
            Err(canary.context(format!("{label} target cleanup also failed: {cleanup:#}")))
        }
    }
}

/// Waits for the target to run and connects the guest framing socket through the authenticated
/// provider endpoint.
async fn connect_to_target(
    control: &Control,
    seal: &KnownTargetSeal,
) -> anyhow::Result<(LaunchedEnvironment, launch::GuestSocket)> {
    let vm = launch::wait_for_state(
        control,
        &seal.target_id,
        &MicrovmState::Running,
        STATE_TIMEOUT,
    )
    .await?;
    let endpoint = vm.endpoint.context("canary target has no endpoint")?;
    let environment = LaunchedEnvironment {
        microvm_id: seal.target_id.clone(),
        endpoint: launch::normalise_endpoint(&endpoint),
        auth_token: control.auth_token(&seal.target_id).await?,
        control_token: seal.control_token.clone(),
    };
    let socket = launch::connect(&environment).await?;
    Ok((environment, socket))
}

/// Executes the sealed request and observes it through to its terminal receipt.
async fn execute_and_observe(
    socket: &mut launch::GuestSocket,
    request_number: &mut u64,
    request: SandboxExecutionRequest,
    what: &str,
) -> anyhow::Result<(OperationRef, TerminalResult)> {
    let receipt = match call(socket, request_number, RequestCall::ExecuteSandbox(request)).await? {
        ResponseReply::ExecuteSandbox(receipt) => receipt,
        _ => bail!("{what} canary execute returned the wrong response variant"),
    };
    let observe: ObserveRequest = serde_json::from_value(serde_json::json!({
        "cursor": "0",
        "operation": receipt.operation,
        "wait_ms": 30_000
    }))?;
    let observation = match call(socket, request_number, RequestCall::Observe(observe)).await? {
        ResponseReply::Observe(observation) => observation,
        _ => bail!("{what} canary observe returned the wrong response variant"),
    };
    let terminal = observation
        .terminal
        .with_context(|| format!("{what} canary did not reach a terminal receipt"))?;
    Ok((observation.operation, terminal))
}

fn require_completed_stdout<'a>(
    terminal: &'a TerminalResult,
    what: &str,
) -> anyhow::Result<&'a str> {
    let diagnostic = terminal_diagnostic(terminal.inline.as_ref());
    ensure!(
        terminal.outcome == TerminalOutcome::Completed,
        "{what} canary command ended as {} with exit code {:?}: {diagnostic}",
        terminal.outcome,
        terminal.exit_code
    );
    terminal
        .inline
        .as_ref()
        .and_then(|value| value.get("stdout"))
        .and_then(|value| value.as_str())
        .with_context(|| format!("{what} canary returned no stdout"))
}

async fn run_restricted_network_canary(
    control: &Control,
    image_arn: &str,
    image_version: &str,
    connector: &ConnectorRef,
    gateway: &GatewayAuthority,
    class: RestrictedClass,
) -> anyhow::Result<()> {
    let label = class.label();
    let network = match class {
        RestrictedClass::None => NetworkCeiling::None,
        RestrictedClass::Allowlist => serde_json::from_value(serde_json::json!({
            "kind": "allowlist",
            "destinations": [{
                "protocol": "tls",
                "host": "example.com",
                "ports": [443]
            }]
        }))?,
    };
    // The release gate intentionally has no KMS signing authority. A syntactically harmless,
    // invalid grant lets the guest configure its ordinary proxy environment while proving that
    // the gateway itself rejects missing and invalid authentication.
    let allowlist_proxy = matches!(class, RestrictedClass::Allowlist).then(|| AllowlistProxy {
        authority: gateway.as_authority(),
        capability: "invalid-release-canary-capability".into(),
    });
    let (seal, payload) = seal_canary_run(
        &format!("{label}-network"),
        class.class(),
        network,
        allowlist_proxy,
        false,
    )?;
    with_canary_target(
        control,
        CanaryLaunch {
            image_arn,
            image_version,
            connector,
        },
        &format!("{label} connector"),
        seal,
        &payload,
        |seal| async move {
            run_restricted_network_on_known_target(control, &seal, gateway, class).await
        },
    )
    .await
}

async fn run_restricted_network_on_known_target(
    control: &Control,
    target: &KnownTargetSeal,
    gateway: &GatewayAuthority,
    class: RestrictedClass,
) -> anyhow::Result<()> {
    let (_environment, mut socket) = connect_to_target(control, target).await?;
    let request = restricted_network_execution(
        &target.operation_id,
        &target.generation,
        &target.root_id,
        &target.session_id,
        gateway,
        class,
    )?;
    let mut request_number = 1u64;
    let what = format!("{} restricted network", class.label());
    let (_, terminal) =
        execute_and_observe(&mut socket, &mut request_number, request, &what).await?;
    let stdout = require_completed_stdout(&terminal, &what)?;
    ensure!(
        stdout.starts_with(&format!(
            "restricted_network_canary=ok class={} ",
            class.label()
        )),
        "restricted network canary returned an unexpected result"
    );
    Ok(())
}

/// Exercises the canonical IPv4 special-use fixtures from inside one exact immutable image.
/// Terraform's exact NACL plan is the authoritative rule proof; connection failures here are only
/// behavioral coverage because an unroutable destination does not identify which layer rejected
/// it. At least one canonical public control must be reachable, so a completely broken connector
/// cannot pass without making release health depend on every third-party fixture staying online.
async fn run_public_network_canary(
    control: &Control,
    cfg: &NetworkBoundaryCanaryConfig,
) -> anyhow::Result<()> {
    let (seal, payload) = seal_canary_run(
        "network",
        ConnectorClass::Public,
        NetworkCeiling::Public,
        None,
        false,
    )?;
    with_canary_target(
        control,
        CanaryLaunch {
            image_arn: &cfg.image_arn,
            image_version: &cfg.image_version,
            connector: cfg.connectors.resolve(ConnectorClass::Public),
        },
        "direct-public network",
        seal,
        &payload,
        |seal| async move {
            run_public_network_on_known_target(control, &seal, &cfg.customer_environment_hosts)
                .await
        },
    )
    .await
}

async fn run_public_network_on_known_target(
    control: &Control,
    target: &KnownTargetSeal,
    customer_environment_hosts: &[String; 2],
) -> anyhow::Result<()> {
    let (_environment, mut socket) = connect_to_target(control, target).await?;
    let request = public_network_execution(
        &target.operation_id,
        &target.generation,
        &target.root_id,
        &target.session_id,
        customer_environment_hosts,
    )?;
    let mut request_number = 1u64;
    let (_, terminal) =
        execute_and_observe(&mut socket, &mut request_number, request, "network").await?;
    let stdout = require_completed_stdout(&terminal, "network")?;
    ensure!(
        stdout.starts_with("network_canary=ok "),
        "network canary returned unexpected stdout: {stdout:?}; terminal detail: {}",
        terminal_diagnostic(terminal.inline.as_ref())
    );
    Ok(())
}

/// Runs against one exact immutable image version. This is deliberately separate from normal
/// construction: every production `RunPayload` sets the canary field to `None`.
pub async fn run_no_respawn_canary(
    control: &Control,
    http: &reqwest::Client,
    cfg: NoRespawnCanaryConfig,
) -> anyhow::Result<()> {
    let (seal, payload) = seal_canary_run(
        "image",
        ConnectorClass::None,
        NetworkCeiling::None,
        None,
        true,
    )?;
    with_canary_target(
        control,
        CanaryLaunch {
            image_arn: &cfg.image_arn,
            image_version: &cfg.image_version,
            connector: &cfg.none_connector,
        },
        "no-respawn image",
        seal,
        &payload,
        |seal| async move { run_on_known_target(control, http, &seal).await },
    )
    .await
}

async fn run_on_known_target(
    control: &Control,
    http: &reqwest::Client,
    target: &KnownTargetSeal,
) -> anyhow::Result<()> {
    // AWS expires idle endpoint connections. Connect after this delay so the gate measures the
    // MicroVM's run-hook stability rather than the lifetime of a silent WebSocket.
    tokio::time::sleep(RUN_HOOK_STABILITY_WINDOW).await;
    let (mut environment, mut socket) = connect_to_target(control, target).await?;
    let request = canary_execution(
        &target.operation_id,
        &target.generation,
        &target.root_id,
        &target.session_id,
    )?;
    let mut request_number = 1u64;
    let (operation, terminal) = execute_and_observe(
        &mut socket,
        &mut request_number,
        request.clone(),
        "no-respawn image",
    )
    .await?;
    let diagnostic = terminal_diagnostic(terminal.inline.as_ref());
    ensure!(
        terminal.outcome == TerminalOutcome::Completed,
        "canary marker effect ended as {} with exit code {:?}: {diagnostic}",
        terminal.outcome,
        terminal.exit_code
    );
    ensure!(
        terminal
            .inline
            .as_ref()
            .and_then(|value| value.get("stdout"))
            .and_then(|value| value.as_str())
            == Some("marker_count=1\n"),
        "canary marker was not written exactly once before the crash"
    );
    let acknowledgement = AcknowledgeTerminalRequest {
        operation,
        terminal_digest: terminal.terminal_digest,
    };
    match call(
        &mut socket,
        &mut request_number,
        RequestCall::AcknowledgeTerminal(acknowledgement.clone()),
    )
    .await?
    {
        ResponseReply::AcknowledgeTerminal(receipt) if receipt.acknowledged => {}
        _ => bail!("image canary terminal acknowledgement was refused"),
    }
    let _ = send(
        &mut socket,
        &mut request_number,
        RequestCall::AcknowledgeTerminal(acknowledgement),
    )
    .await;
    drop(socket);

    assert_persistent_502(control, http, &environment, "after deliberate crash").await?;
    if !transition_and_wait_or_confirm_gone(control, &target.target_id, MicrovmState::Suspended)
        .await?
    {
        return Ok(());
    }
    if !transition_and_wait_or_confirm_gone(control, &target.target_id, MicrovmState::Running)
        .await?
    {
        return Ok(());
    }
    environment.auth_token = control.auth_token(&target.target_id).await?;
    assert_persistent_502(control, http, &environment, "after suspend/resume").await?;

    // Attempt the exact operation/digest again. Persistent provider 502 must prevent transport
    // admission, which is the physical proof that the already-one-line marker cannot gain a
    // second line. Any successful handshake is a re-arm and blocks image promotion immediately.
    match launch::connect(&environment).await {
        Err(GuestConnectError::Http(502)) => {}
        Err(error) => bail!("exact replay did not receive the expected endpoint 502: {error}"),
        Ok(mut socket) => {
            let _ = call(
                &mut socket,
                &mut request_number,
                RequestCall::ExecuteSandbox(request.clone()),
            )
            .await;
            bail!("physical generation accepted an exact replay after supervisor loss")
        }
    }
    Ok(())
}

fn canary_execution(
    operation_id: &str,
    generation: &str,
    root_id: &str,
    session_id: &str,
) -> anyhow::Result<SandboxExecutionRequest> {
    let mut request: SandboxExecutionRequest = serde_json::from_value(serde_json::json!({
        "execution_id": operation_id,
        "expected_generation": generation,
        "input": {
            "command": "printf 'marker\\n' >> /workspace/image-canary-effect; sync /workspace/image-canary-effect; printf 'marker_count=%s\\n' \"$(wc -l < /workspace/image-canary-effect)\"",
            "cwd": "/workspace",
            "interactive": false
        },
        "network": {"kind": "none"},
        "request_digest": "0".repeat(64),
        "resources": {"max_output_bytes": 4_096, "timeout_ms": 30_000},
        "target": {
            "binding_ref": "image-canary-binding",
            "kind": "additional",
            "root_id": root_id,
            "sandbox_id": "image-canary-sandbox",
            "session_id": session_id
        }
    }))?;
    request.request_digest = sandbox_execution_request_digest(&request);
    Ok(request)
}

/// Special-use IPv4 destinations whose traffic actually reaches the plane-owned VPC connector.
/// Lambda MicroVM loopback and IMDS are provider-local sockets: they do not traverse that
/// connector and therefore cannot prove its NACL policy. The image separately proves that the
/// loopback control listener requires its generation bearer, and `RunMicrovm` deliberately omits
/// an execution role so the provider-local IMDS endpoint has no AWS credentials to return.
fn connector_routed_special_use_ipv4_fixtures() -> Vec<&'static str> {
    brain_protocol::network::SPECIAL_USE_FIXTURES
        .iter()
        .filter_map(|&(address, _)| {
            address
                .parse::<Ipv4Addr>()
                .ok()
                .map(|parsed| (address, parsed))
        })
        .filter(|(_, address)| {
            !address.is_loopback() && *address != Ipv4Addr::new(169, 254, 169, 254)
        })
        .map(|(address, _)| address)
        .collect()
}

fn shell_words(values: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    values
        .into_iter()
        .map(|value| format!("'{}'", value.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn restricted_network_execution(
    operation_id: &str,
    generation: &str,
    root_id: &str,
    session_id: &str,
    gateway: &GatewayAuthority,
    class: RestrictedClass,
) -> anyhow::Result<SandboxExecutionRequest> {
    ensure!(
        gateway.host().parse::<Ipv4Addr>().is_ok(),
        "release canary requires the platform's fixed IPv4 gateway authority"
    );
    let denied: Vec<&str> = connector_routed_special_use_ipv4_fixtures()
        .into_iter()
        .filter(|address| *address != gateway.host())
        .collect();
    let controls: Vec<&str> = brain_protocol::network::PUBLIC_UNICAST_FIXTURES
        .iter()
        .copied()
        .filter(|address| address.parse::<Ipv4Addr>().is_ok())
        .collect();
    ensure!(!denied.is_empty(), "canonical denied fixture set is empty");
    ensure!(
        !controls.is_empty(),
        "canonical public control set is empty"
    );
    let label = class.label();
    let (network, require_gateway) = match class {
        RestrictedClass::None => (serde_json::json!({"kind": "none"}), false),
        RestrictedClass::Allowlist => (
            serde_json::json!({
                "kind": "allowlist",
                "destinations": [{
                    "protocol": "tls",
                    "host": "example.com",
                    "ports": [443]
                }]
            }),
            true,
        ),
    };

    let command = include_str!("canary/restricted-network.sh")
        .replace("__CLASS__", label)
        .replace("__DENIED__", &shell_words(&denied))
        .replace("__CONTROLS__", &shell_words(&controls))
        .replace("__GATEWAY_HOST__", gateway.host())
        .replace("__GATEWAY_PORT__", &gateway.port().get().to_string())
        .replace(
            "__REQUIRE_GATEWAY__",
            if require_gateway { "1" } else { "0" },
        );
    let mut request: SandboxExecutionRequest = serde_json::from_value(serde_json::json!({
        "execution_id": operation_id,
        "expected_generation": generation,
        "input": {
            "command": command,
            "cwd": "/workspace",
            "interactive": false
        },
        "network": network,
        "request_digest": "0".repeat(64),
        "resources": {"max_output_bytes": 4_096, "timeout_ms": 30_000},
        "target": {
            "binding_ref": format!("{label}-network-canary-binding"),
            "kind": "additional",
            "root_id": root_id,
            "sandbox_id": format!("{label}-network-canary-sandbox"),
            "session_id": session_id
        }
    }))?;
    request.request_digest = sandbox_execution_request_digest(&request);
    Ok(request)
}

fn public_network_execution(
    operation_id: &str,
    generation: &str,
    root_id: &str,
    session_id: &str,
    customer_environment_hosts: &[String; 2],
) -> anyhow::Result<SandboxExecutionRequest> {
    let denied = connector_routed_special_use_ipv4_fixtures();
    let controls: Vec<&str> = brain_protocol::network::PUBLIC_UNICAST_FIXTURES
        .iter()
        .copied()
        .filter(|address| address.parse::<Ipv4Addr>().is_ok())
        .collect();
    ensure!(!denied.is_empty(), "canonical denied fixture set is empty");
    ensure!(
        !controls.is_empty(),
        "canonical public control set is empty"
    );
    let http_surfaces = [
        serde_json::json!({"host": "aex.dev", "path": "/"}),
        serde_json::json!({"host": "api.aex.dev", "path": "/v1/rates"}),
        serde_json::json!({"host": "api-dev.aex.dev", "path": "/v1/rates"}),
    ];

    let command = include_str!("canary/public-network.sh")
        .replace("__DENIED__", &shell_words(&denied))
        .replace("__CONTROLS__", &shell_words(&controls))
        .replace(
            "__HTTP_SURFACES__",
            &shell_words(http_surfaces.iter().map(|surface| {
                format!(
                    "{}|{}",
                    surface["host"].as_str().expect("static host"),
                    surface["path"].as_str().expect("static path")
                )
            })),
        )
        .replace(
            "__CUSTOMER_ENVIRONMENT_HOSTS__",
            &shell_words(customer_environment_hosts),
        );
    let mut request: SandboxExecutionRequest = serde_json::from_value(serde_json::json!({
        "execution_id": operation_id,
        "expected_generation": generation,
        "input": {
            "command": command,
            "cwd": "/workspace",
            "interactive": false
        },
        "network": {"kind": "public"},
        "request_digest": "0".repeat(64),
        "resources": {"max_output_bytes": 4_096, "timeout_ms": 30_000},
        "target": {
            "binding_ref": "network-canary-binding",
            "kind": "additional",
            "root_id": root_id,
            "sandbox_id": "network-canary-sandbox",
            "session_id": session_id
        }
    }))?;
    request.request_digest = sandbox_execution_request_digest(&request);
    Ok(request)
}

fn validate_customer_environment_hosts(hosts: &[String; 2], region: &str) -> anyhow::Result<()> {
    ensure!(
        hosts[0] != hosts[1],
        "customer Environment hosts must be distinct"
    );
    let suffix = format!(".execute-api.{region}.amazonaws.com");
    for host in hosts {
        ensure!(
            host.len() <= 253
                && host == host.trim()
                && host.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'.' | b'-')
                })
                && host.ends_with(&suffix)
                && host.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && label
                            .as_bytes()
                            .first()
                            .is_some_and(u8::is_ascii_alphanumeric)
                        && label
                            .as_bytes()
                            .last()
                            .is_some_and(u8::is_ascii_alphanumeric)
                }),
            "invalid customer Environment API Gateway host"
        );
    }
    Ok(())
}

fn terminal_diagnostic(inline: Option<&serde_json::Value>) -> &str {
    inline
        .and_then(|value| {
            ["error", "stderr", "stdout"]
                .into_iter()
                .find_map(|field| value.get(field).and_then(serde_json::Value::as_str))
        })
        .filter(|value| !value.is_empty())
        .unwrap_or("no terminal detail")
}

async fn call(
    socket: &mut launch::GuestSocket,
    request_number: &mut u64,
    call: RequestCall,
) -> anyhow::Result<ResponseReply> {
    let request_id = send(socket, request_number, call).await?;
    while let Some(message) = socket.next().await {
        let text = match message? {
            Message::Text(text) => text.to_string(),
            Message::Binary(bytes) => String::from_utf8(bytes.to_vec())?,
            Message::Ping(bytes) => {
                socket.send(Message::Pong(bytes)).await?;
                continue;
            }
            Message::Pong(_) | Message::Frame(_) => continue,
            Message::Close(_) => bail!("canary guest closed before its receipt"),
        };
        let response: ResponseFrame = serde_json::from_str(&text)?;
        if response.request_id != request_id {
            continue;
        }
        return response.result.map_err(|error| {
            anyhow::anyhow!(
                "canary guest refused the request: {}: {}",
                error.code,
                error.message.as_str()
            )
        });
    }
    bail!("canary guest connection ended before its receipt")
}

async fn send(
    socket: &mut launch::GuestSocket,
    request_number: &mut u64,
    call: RequestCall,
) -> anyhow::Result<String> {
    let request_id = format!("image-canary-request-{request_number}");
    *request_number += 1;
    let frame = RequestFrame {
        request_id: request_id.clone(),
        contract_digest: ENVIRONMENT_CONTRACT_DIGEST.trim().into(),
        call,
    };
    socket
        .send(Message::Text(serde_json::to_string(&frame)?.into()))
        .await?;
    Ok(request_id)
}

async fn launch_canary_target(
    control: &Control,
    image_arn: &str,
    image_version: &str,
    run_hook_payload: &str,
    client_token: &str,
    connector: &ConnectorRef,
) -> Result<Microvm, ControlError> {
    let request = control.exact_run_request(
        image_arn,
        image_version,
        run_hook_payload,
        client_token,
        connector,
        crate::TargetLifetime::default(),
    );
    let deadline = tokio::time::Instant::now() + STATE_TIMEOUT;
    loop {
        match control.run_exact(&request).await {
            Ok(vm) => return Ok(vm),
            Err(
                ControlError::Unknown(_) | ControlError::Retryable(_) | ControlError::Throttled(_),
            ) if tokio::time::Instant::now() < deadline => {
                // RunMicrovm's documented client-token contract makes an exact replay the only
                // safe way to recover the target id after a lost response. `run_exact` applies
                // the configured provider token-bucket before each attempt.
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

async fn assert_persistent_502(
    control: &Control,
    http: &reqwest::Client,
    environment: &LaunchedEnvironment,
    phase: &str,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + STATE_TIMEOUT;
    let mut consecutive = 0usize;
    while tokio::time::Instant::now() < deadline {
        // The state read only decorates the probe log; a transient control-plane blip must not
        // abort a destructive gate that has a live VM outstanding.
        let state = match control.get(&environment.microvm_id).await {
            Ok(vm) => Some(vm.state),
            Err(ControlError::Retryable(_) | ControlError::Throttled(_)) => None,
            Err(error) => return Err(error.into()),
        };
        // A transport failure is not a 502 observation: the consecutive run restarts and only
        // the deadline can fail the gate.
        let status = match http
            .get(format!("{}/", environment.endpoint))
            .header(AUTH_HEADER, &environment.auth_token)
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
        {
            Ok(response) => Some(response.status()),
            Err(error) => {
                tracing::info!(microvm = %environment.microvm_id, %error, phase, "no-respawn canary probe transport error");
                consecutive = 0;
                None
            }
        };
        tracing::info!(microvm = %environment.microvm_id, ?state, ?status, phase, "no-respawn canary probe");
        if let Some(status) = status {
            if status.as_u16() == 502 {
                consecutive += 1;
                if consecutive == REQUIRED_CONSECUTIVE_502 {
                    return Ok(());
                }
            } else {
                ensure!(
                    !status.is_success(),
                    "image canary endpoint rearmed during {phase}"
                );
                consecutive = 0;
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    bail!("image canary did not produce repeated endpoint 502 during {phase}")
}

/// A provider may terminate a MicroVM whose main supervisor has exited instead of retaining it for
/// suspend/resume. That terminal state is already definitive physical no-respawn proof. When the
/// provider keeps the VM alive, retain the stronger suspend/resume + exact-replay coverage.
async fn transition_and_wait_or_confirm_gone(
    control: &Control,
    target_id: &str,
    wanted: MicrovmState,
) -> anyhow::Result<bool> {
    let effect = match wanted {
        MicrovmState::Suspended => control.suspend(target_id).await,
        MicrovmState::Running => control.resume(target_id).await,
        _ => bail!("unsupported canary transition {wanted:?}"),
    };
    match effect {
        Ok(()) | Err(ControlError::Unknown(_)) => {}
        Err(ControlError::Gone(_)) => return Ok(false),
        Err(error) => return Err(error.into()),
    }
    match launch::wait_for_state(control, target_id, &wanted, STATE_TIMEOUT).await {
        Ok(_) => Ok(true),
        Err(wait_error) => match control.get(target_id).await {
            Ok(vm) if is_gone(&vm.state) => {
                tracing::info!(microvm = target_id, state = ?vm.state, "no-respawn canary confirmed provider terminal state");
                Ok(false)
            }
            Err(ControlError::Gone(_)) => Ok(false),
            Ok(_) | Err(_) => Err(wait_error),
        },
    }
}

async fn terminate_known_target(control: &Control, target_id: &str) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + STATE_TIMEOUT;
    loop {
        if tokio::time::Instant::now() >= deadline {
            bail!("canary target termination was not confirmed within {STATE_TIMEOUT:?}");
        }
        match control.get(target_id).await {
            Ok(vm) if is_terminated(&vm.state) => return Ok(()),
            Err(ControlError::Gone(_)) => return Ok(()),
            Ok(vm) if vm.state == MicrovmState::Terminating => {}
            Ok(_) => match control.terminate(target_id).await {
                Ok(())
                | Err(
                    ControlError::Gone(_)
                    | ControlError::Unknown(_)
                    | ControlError::Retryable(_)
                    | ControlError::Throttled(_),
                ) => {}
                Err(error) => return Err(error.into()),
            },
            Err(ControlError::Retryable(_) | ControlError::Throttled(_)) => {}
            Err(error) => return Err(error.into()),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn canary_control_token() -> ControlToken {
    ControlToken::new(format!(
        "control-{}",
        hex::encode(rand::random::<[u8; 32]>())
    ))
    .expect("random canary control token satisfies its exact grammar")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canary_request_digest_is_exact_and_marker_command_is_single_effect() {
        let request = canary_execution(
            "image-canary-operation",
            "image-canary-generation",
            "image-canary-root",
            "image-canary-session",
        )
        .unwrap();
        assert_eq!(
            request.request_digest,
            sandbox_execution_request_digest(&request)
        );
        assert!(request.input.command.contains("printf 'marker\\n' >>"));
        assert!(
            request
                .input
                .command
                .contains("sync /workspace/image-canary-effect")
        );
    }

    #[test]
    fn public_network_canary_uses_every_connector_routed_ipv4_fixture_and_exact_digest() {
        let request = public_network_execution(
            "network-canary-operation",
            "network-canary-generation",
            "network-canary-root",
            "network-canary-session",
            &[
                "dev123.execute-api.us-east-1.amazonaws.com".into(),
                "prd456.execute-api.us-east-1.amazonaws.com".into(),
            ],
        )
        .unwrap();
        assert_eq!(
            request.request_digest,
            sandbox_execution_request_digest(&request)
        );
        assert!(request.input.command.starts_with("set -euo pipefail"));
        assert!(!request.input.command.contains("bash <<"));
        let denied = connector_routed_special_use_ipv4_fixtures();
        for &(address, _) in brain_protocol::network::SPECIAL_USE_FIXTURES {
            if address.parse::<Ipv4Addr>().is_ok() {
                assert_eq!(
                    request.input.command.contains(address),
                    denied.contains(&address),
                    "{address}"
                );
            }
        }
        for &address in brain_protocol::network::PUBLIC_UNICAST_FIXTURES {
            if address.parse::<Ipv4Addr>().is_ok() {
                assert!(request.input.command.contains(address), "{address}");
            }
        }
        assert!(request.input.command.contains("reachable_special"));
        assert!(request.input.command.contains("reachable_controls"));
        assert!(request.input.command.contains("--parallel-max 32"));
        assert!(!request.input.command.contains("mktemp"));
        assert!(!request.input.command.contains("node --input-type"));
        assert!(request.input.command.contains("Aex HTTPS surface"));
        assert!(request.input.command.contains("checkip.amazonaws.com"));
        assert!(request.input.command.contains("observed_public_source"));
        for host in [
            "aex.dev",
            "api.aex.dev",
            "api-dev.aex.dev",
            "dev123.execute-api.us-east-1.amazonaws.com",
            "prd456.execute-api.us-east-1.amazonaws.com",
        ] {
            assert!(request.input.command.contains(host), "{host}");
        }
        assert!(request.input.command.contains("Sec-WebSocket-Key"));
        assert!(
            request
                .input
                .command
                .contains("@connections/L0SM9cOFvHcCIhw%3D")
        );
        assert!(!request.input.command.contains("aex-network-canary"));
    }

    #[test]
    fn restricted_connector_canaries_cover_dns_direct_tcp_and_gateway_auth() {
        let gateway = GatewayAuthority::parse("10.42.0.10:8443").unwrap();
        for class in [RestrictedClass::None, RestrictedClass::Allowlist] {
            let request = restricted_network_execution(
                "restricted-network-operation",
                "restricted-network-generation",
                "restricted-network-root",
                "restricted-network-session",
                &gateway,
                class,
            )
            .unwrap();
            assert_eq!(
                request.request_digest,
                sandbox_execution_request_digest(&request)
            );
            assert!(request.input.command.starts_with("set -euo pipefail"));
            assert!(!request.input.command.contains("bash <<"));
            assert!(request.input.command.contains("timeout 3 getent ahostsv4"));
            assert!(request.input.command.contains("--parallel-max 32"));
            assert!(!request.input.command.contains("mktemp"));
            assert!(request.input.command.contains("10.42.0.10"));
            let denied = connector_routed_special_use_ipv4_fixtures();
            for &(address, _) in brain_protocol::network::SPECIAL_USE_FIXTURES {
                if address.parse::<Ipv4Addr>().is_ok() {
                    assert_eq!(
                        request.input.command.contains(address),
                        denied.contains(&address) && address != gateway.host(),
                        "{address}"
                    );
                }
            }
            for &address in brain_protocol::network::PUBLIC_UNICAST_FIXTURES {
                if address.parse::<Ipv4Addr>().is_ok() {
                    assert!(request.input.command.contains(address), "{address}");
                }
            }
            match class {
                RestrictedClass::None => {
                    assert!(matches!(request.network, NetworkCeiling::None));
                    assert!(request.input.command.contains("require_gateway=0"));
                }
                RestrictedClass::Allowlist => {
                    assert!(matches!(request.network, NetworkCeiling::Allowlist(_)));
                    assert!(request.input.command.contains("require_gateway=1"));
                    assert!(request.input.command.contains("--proxytunnel"));
                    assert!(request.input.command.contains("%{http_connect}"));
                    assert!(!request.input.command.contains("/dev/tcp"));
                    assert!(request.input.command.contains("$unauthenticated == 407"));
                    assert!(request.input.command.contains("$invalid == 403"));
                }
            }
        }
    }

    #[test]
    fn customer_environment_canary_hosts_are_two_distinct_regional_api_gateway_hosts() {
        assert!(
            validate_customer_environment_hosts(
                &[
                    "dev123.execute-api.us-east-1.amazonaws.com".into(),
                    "prd456.execute-api.us-east-1.amazonaws.com".into(),
                ],
                "us-east-1"
            )
            .is_ok()
        );
        for invalid in [
            [
                "dev123.execute-api.us-east-1.amazonaws.com".into(),
                "dev123.execute-api.us-east-1.amazonaws.com".into(),
            ],
            [
                "https://dev123.execute-api.us-east-1.amazonaws.com/v1".into(),
                "prd456.execute-api.us-east-1.amazonaws.com".into(),
            ],
            [
                "dev123.execute-api.eu-west-1.amazonaws.com".into(),
                "prd456.execute-api.us-east-1.amazonaws.com".into(),
            ],
        ] {
            assert!(validate_customer_environment_hosts(&invalid, "us-east-1").is_err());
        }
    }

    #[test]
    fn failed_network_canary_reports_the_bounded_terminal_detail() {
        let stderr = serde_json::json!({"stdout": "", "stderr": "DNS unexpectedly resolved"});
        let explicit = serde_json::json!({"error": "sandbox deadline", "stderr": "ignored"});
        assert_eq!(
            terminal_diagnostic(Some(&stderr)),
            "DNS unexpectedly resolved"
        );
        assert_eq!(terminal_diagnostic(Some(&explicit)), "sandbox deadline");
        assert_eq!(terminal_diagnostic(None), "no terminal detail");
    }
}
