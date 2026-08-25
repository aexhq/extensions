use super::*;
#[cfg(unix)]
use brain_protocol::contract::{sandbox_execution_request_digest, write_stdin_request_digest};
#[cfg(unix)]
use brain_protocol::environment::{ObserveRequest, SandboxExecutionRequest, WriteStdinRequest};
use environment_wire::{AllowlistProxy, InstallBundleMetadata, RunPayload};

fn run_payload(network: NetworkCeiling) -> RunPayload {
    RunPayload {
        contract_digest: ENVIRONMENT_CONTRACT_DIGEST.trim().into(),
        generation: "generation-1".into(),
        expires_at_ms: wall_ms() + MAX_TARGET_LIFETIME_MS,
        root_id: "root-1".into(),
        owner_session_id: "session-1".into(),
        connector: match network {
            NetworkCeiling::None => ConnectorClass::None,
            NetworkCeiling::Public => ConnectorClass::Public,
            NetworkCeiling::Allowlist(_) => ConnectorClass::Allowlist,
        },
        resource_class: "microvm-1gb".into(),
        resources: serde_json::from_value(serde_json::json!({
            "max_output_bytes": 65536,
            "timeout_ms": 60000
        }))
        .unwrap(),
        control_token: environment_core::materialization::ControlToken::new(format!(
            "control-{}",
            "a".repeat(64)
        ))
        .expect("test control token"),
        allowlist_proxy: matches!(network, NetworkCeiling::Allowlist(_)).then(|| AllowlistProxy {
            authority: "10.0.0.10:8443".into(),
            capability: "opaque-capability".into(),
        }),
        canary_exit_after_operation_id: None,
        network,
    }
}

fn sandbox_identity() -> ToolIdentity {
    ToolIdentity {
        uid: 1_000,
        gid: 1_000,
        supervisor_uid: 1_001,
    }
}

fn default_file_target() -> SandboxTarget {
    serde_json::from_value(serde_json::json!({
        "binding_ref": "file-binding-1",
        "kind": "environment",
        "root_id": "root-1",
        "session_id": "session-1"
    }))
    .unwrap()
}

fn file_effect_identity(operation_id: &str, digest: char) -> FileEffectIdentity {
    FileEffectIdentity {
        kind: FileEffectKind::Write,
        operation_id: operation_id.into(),
        request_digest: digest.to_string().repeat(64),
    }
}

#[tokio::test]
async fn only_the_exact_generation_control_bearer_is_authorized() {
    let directory = tempfile::tempdir().unwrap();
    let environment = Environment::new(Config::for_test(directory.path())).unwrap();
    let payload = run_payload(NetworkCeiling::None);
    let exact = payload.control_token.expose().to_owned();
    environment.arm("target-1".into(), payload).await.unwrap();

    assert!(!environment.control_authorized(None).await);
    assert!(!environment.control_authorized(Some("")).await);
    assert!(
        !environment
            .control_authorized(Some(&format!("control-{}", "b".repeat(64))))
            .await
    );
    assert!(environment.control_authorized(Some(&exact)).await);
}

#[test]
fn managed_binding_uids_are_bounded_exact_and_never_alias() {
    let mut registry = BindingIdentityRegistry::with_bounds(65_536, 1_000_000, 2);
    let first = registry
        .allocate("binding-a", Some(sandbox_identity()))
        .unwrap()
        .unwrap();
    assert!((65_536..1_065_536).contains(&first.uid));
    assert_eq!(
        registry
            .allocate("binding-a", Some(sandbox_identity()))
            .unwrap(),
        Some(first),
    );

    // A one-element uid range makes a distinct hash collision deterministic. It is a
    // permanent binding conflict and never aliases the two secret subsets.
    let mut collision = BindingIdentityRegistry::with_bounds(65_536, 1, 2);
    collision
        .allocate("binding-a", Some(sandbox_identity()))
        .unwrap();
    let error = collision
        .allocate("binding-b", Some(sandbox_identity()))
        .unwrap_err();
    assert_eq!(error.code, EnvironmentErrorCode::BindingConflict);
    assert!(!error.retryable);

    let mut exhausted = BindingIdentityRegistry::with_bounds(65_536, 1_000_000, 1);
    exhausted
        .allocate("binding-a", Some(sandbox_identity()))
        .unwrap();
    let error = exhausted
        .allocate("binding-b", Some(sandbox_identity()))
        .unwrap_err();
    assert_eq!(error.code, EnvironmentErrorCode::ResourceExhausted);
    assert!(!error.retryable);
}

#[test]
fn operation_receipts_are_stable_per_operation_and_distinct_from_target_identity() {
    let digest = "a".repeat(64);
    let first = operation_receipt_ref("operation-1", &digest, "target-1", "generation-1").unwrap();
    let replay = operation_receipt_ref("operation-1", &digest, "target-1", "generation-1").unwrap();
    let second = operation_receipt_ref("operation-2", &digest, "target-1", "generation-1").unwrap();

    assert_eq!(first, replay);
    assert_ne!(first, second);
    assert_ne!(first.as_str(), "target-1");
}

#[tokio::test]
async fn file_write_lost_success_replays_and_conflict_never_mutates_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::for_test(directory.path());
    let workspace = config.workspace.clone();
    let environment = Environment::new(config).unwrap();
    environment
        .arm("target-1".into(), run_payload(NetworkCeiling::None))
        .await
        .unwrap();

    let identity = file_effect_identity("file-operation-1", 'a');
    assert!(matches!(
        environment
            .reserve_file_effect(identity.clone())
            .await
            .unwrap(),
        FileEffectReservation::New
    ));
    let mut request = GuestFileWriteRequest {
        effect: identity.clone(),
        expected_generation: "generation-1".into(),
        overwrite: false,
        path: "/workspace/result.txt".into(),
        source: GuestFileWriteSource::Inline {
            content_base64: base64::engine::general_purpose::STANDARD.encode(b"first"),
        },
        target: default_file_target(),
    };
    let FileEffectStoredResult::Write(first) =
        environment.write_file(request.clone()).await.unwrap()
    else {
        panic!("file write returned a copy result");
    };
    assert!(!first.replayed);
    assert_eq!(
        std::fs::read(workspace.join("result.txt")).unwrap(),
        b"first"
    );

    // Model a successful mutation whose response was lost. Even a different private-wire
    // payload carrying the retained exact identity cannot enter the mutation body again.
    request.overwrite = true;
    request.source = GuestFileWriteSource::Inline {
        content_base64: base64::engine::general_purpose::STANDARD.encode(b"second"),
    };
    let FileEffectStoredResult::Write(replayed) = environment.write_file(request).await.unwrap()
    else {
        panic!("file write replay returned a copy result");
    };
    assert!(replayed.replayed);
    assert_eq!(
        std::fs::read(workspace.join("result.txt")).unwrap(),
        b"first"
    );

    let conflict = environment
        .reserve_file_effect(file_effect_identity("file-operation-1", 'b'))
        .await
        .unwrap_err();
    assert_eq!(conflict.code, EnvironmentErrorCode::BindingConflict);
    assert!(!conflict.retryable);
    assert_eq!(
        std::fs::read(workspace.join("result.txt")).unwrap(),
        b"first"
    );
}

#[tokio::test]
async fn file_write_intent_only_restart_is_unknown_and_never_mutates_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::for_test(directory.path());
    let workspace = config.workspace.clone();
    let identity = file_effect_identity("file-operation-restart", 'a');
    {
        let environment = Environment::new(config.clone()).unwrap();
        environment
            .arm("target-1".into(), run_payload(NetworkCeiling::None))
            .await
            .unwrap();
        assert!(matches!(
            environment
                .reserve_file_effect(identity.clone())
                .await
                .unwrap(),
            FileEffectReservation::New
        ));
    }

    let restarted = Environment::new(config).unwrap();
    restarted
        .arm("target-1".into(), run_payload(NetworkCeiling::None))
        .await
        .unwrap();
    let error = restarted.reserve_file_effect(identity).await.unwrap_err();
    assert_eq!(error.code, EnvironmentErrorCode::OperationUnknown);
    assert!(!error.retryable);
    assert!(!workspace.join("result.txt").exists());
}

#[test]
fn exact_max_inline_terminal_fits_the_reserved_full_observation() {
    let inline =
        serde_json::Value::String("x".repeat(brain_protocol::MAX_TOOL_TERMINAL_INLINE_BYTES - 2));
    assert!(terminal_inline_fits(&inline));
    let mut terminal = TerminalResult {
        duration_ms: Some(u64::MAX),
        exit_code: Some(i64::MIN),
        inline: Some(inline),
        is_error: false,
        object: None,
        outcome: TerminalOutcome::Completed,
        terminal_digest: "0".repeat(64).parse().unwrap(),
    };
    terminal.terminal_digest = terminal_result_digest(&terminal);
    let observation: OperationObservation = serde_json::from_value(serde_json::json!({
        "next_cursor": "c".repeat(256),
        "operation": {
            "generation": "g".repeat(128),
            "operation_id": "o".repeat(128),
            "receipt_ref": "r".repeat(128),
            "request_digest": "a".repeat(64),
            "target": {
                "binding_ref": "b".repeat(128),
                "kind": "environment",
                "root_id": "t".repeat(128),
                "session_id": "s".repeat(128)
            },
            "target_ref": "p".repeat(128)
        },
        "output": [],
        "state": "terminal",
        "target": {
            "expires_at_ms": u64::MAX,
            "generation": "g".repeat(128),
            "target_ref": "p".repeat(128)
        },
        "terminal": terminal
    }))
    .unwrap();
    let bytes = serde_json::to_vec(&observation).unwrap();
    assert!(
        bytes.len() <= TERMINAL_ENVELOPE_BYTES,
        "max canonical inline plus maximum receipt fields encoded to {} bytes, above the {}-byte reservation",
        bytes.len(),
        TERMINAL_ENVELOPE_BYTES
    );
}

async fn prepared_environment() -> (tempfile::TempDir, Arc<Environment>, String) {
    let directory = tempfile::tempdir().unwrap();
    let environment = Environment::new(Config::for_test(directory.path())).unwrap();
    environment
        .arm("mvm-1".into(), run_payload(NetworkCeiling::None))
        .await
        .unwrap();
    let bytes = br#"export default {kind:'brain.tool-runtime',name:'fixture',execute: async () => ({ok:true})};"#;
    let digest = hex::encode(Sha256::digest(bytes));
    let node = b"#!/bin/sh\nexit 0\n";
    let node_digest = hex::encode(Sha256::digest(node));
    let descriptor: BundleDescriptor = serde_json::from_value(serde_json::json!({
        "bundle_digest": "b".repeat(64),
        "bytes": bytes.len() + node.len(),
        "contract_digest": "a".repeat(64),
        "layers": [{
            "digest": node_digest,
            "bytes": node.len(),
            "media_type": "application/javascript+esm",
            "mount_path": "/runtime/bin/node",
            "unpack": "file",
            "object": {"bytes": node.len(), "object_id": "object-node", "sha256": node_digest}
        }, {
            "digest": digest,
            "bytes": bytes.len(),
            "media_type": "application/javascript+esm",
            "mount_path": "/tool/runtime.mjs",
            "unpack": "file",
            "object": {"bytes": bytes.len(), "object_id": "object-code", "sha256": digest}
        }],
        "required_env": ["FIXTURE_SECRET"],
        "target": "linux-arm64",
        "execute_path": "/tool/runtime.mjs",
        "environment_name": "workspace",
        "tool_name": "fixture"
    }))
    .unwrap();
    environment
        .install_bundle(
            InstallBundleMetadata {
                descriptor: descriptor.clone(),
                layer_digest: node_digest,
            },
            node,
        )
        .await
        .unwrap();
    environment
        .install_bundle(
            InstallBundleMetadata {
                descriptor: descriptor.clone(),
                layer_digest: digest.clone(),
            },
            bytes,
        )
        .await
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let installed =
            std::fs::metadata(environment.cfg.tool_dir.join("layers").join(&digest)).unwrap();
        assert_eq!(installed.permissions().mode() & 0o777, 0o640);
    }
    let binding: SealedBinding = serde_json::from_value(serde_json::json!({
        "binding_id": "binding-1",
        "bundle": descriptor,
        "capability": "fixture",
        "configuration": {},
        "contract_digest": "a".repeat(64),
        "extension": "@aexhq/env-aws-microvm",
        "implementation_identity": "b".repeat(64),
        "policy_digest": "c".repeat(64),
        "environment_name": "workspace",
        "profile": {
            "kind":"computer",
            "platform":"linux-arm64",
            "network":"allowlist",
            "recovery":"retained"
        },
        "protocol":"environment/v1",
        "required_capabilities": ["execution"],
        "root_id": "root-1",
        "session_id": "session-1"
    }))
    .unwrap();
    environment
        .install_binding(InstallBindingRequest {
            binding_ref: "binding-ref-1".into(),
            binding,
        })
        .await
        .unwrap();
    (directory, environment, digest)
}

#[tokio::test]
async fn root_network_and_resource_seals_cannot_be_widened() {
    let directory = tempfile::tempdir().unwrap();
    let environment = Environment::new(Config::for_test(directory.path())).unwrap();
    environment
        .arm("mvm-1".into(), run_payload(NetworkCeiling::None))
        .await
        .unwrap();
    let error = environment
        .arm("mvm-1".into(), run_payload(NetworkCeiling::Public))
        .await
        .unwrap_err();
    assert_eq!(error.code, EnvironmentErrorCode::GenerationConflict);
    let status = environment.runtime_status().await.unwrap();
    assert_eq!(status.connector, ConnectorClass::None);
}

#[tokio::test]
async fn secrets_are_declared_exact_replay_only_and_absent_from_receipts() {
    let (_directory, environment, _digest) = prepared_environment().await;
    let secret = "never-print-this-value";
    let request = || InstallSecretsRequest {
        session_id: "session-1".into(),
        generation: "generation-1".into(),
        env_names: vec!["FIXTURE_SECRET".into(), "FUTURE_SECRET".into()],
        values: HashMap::from([
            ("FIXTURE_SECRET".into(), secret.into()),
            ("FUTURE_SECRET".into(), "future-value".into()),
        ]),
    };
    let first = environment.install_secrets(request()).await.unwrap();
    assert!(!first.replayed);
    assert!(
        environment
            .install_secrets(request())
            .await
            .unwrap()
            .replayed
    );
    let conflict = environment
        .install_secrets(InstallSecretsRequest {
            session_id: "session-1".into(),
            generation: "generation-1".into(),
            env_names: vec!["FIXTURE_SECRET".into(), "FUTURE_SECRET".into()],
            values: HashMap::from([
                ("FIXTURE_SECRET".into(), "different".into()),
                ("FUTURE_SECRET".into(), "future-value".into()),
            ]),
        })
        .await
        .unwrap_err();
    assert_eq!(conflict.code, EnvironmentErrorCode::GenerationConflict);
    let status = serde_json::to_string(&environment.runtime_status().await).unwrap();
    let receipt = serde_json::to_string(&first).unwrap();
    assert!(!status.contains(secret));
    assert!(!receipt.contains(secret));
}

#[tokio::test]
async fn guest_repeats_the_exact_brain_secret_document_boundary() {
    let exact_directory = tempfile::tempdir().unwrap();
    let exact_environment = Environment::new(Config::for_test(exact_directory.path())).unwrap();
    exact_environment
        .arm("mvm-exact".into(), run_payload(NetworkCeiling::None))
        .await
        .unwrap();
    let exact_value = format!("{}aaaaaaaa", "é".repeat(2040));
    let exact = InstallSecretsRequest {
        session_id: "session-exact".into(),
        generation: "generation-1".into(),
        env_names: vec!["A".into()],
        values: HashMap::from([("A".into(), exact_value)]),
    };
    assert_eq!(
        serde_jcs::to_vec(&exact.values).unwrap().len(),
        brain_protocol::MAX_SESSION_SECRET_DOCUMENT_BYTES
    );
    exact_environment.install_secrets(exact).await.unwrap();

    let oversized_directory = tempfile::tempdir().unwrap();
    let oversized_environment =
        Environment::new(Config::for_test(oversized_directory.path())).unwrap();
    oversized_environment
        .arm("mvm-oversized".into(), run_payload(NetworkCeiling::None))
        .await
        .unwrap();
    let oversized_value = format!("{}aaaaaaaa€", "é".repeat(2039));
    let oversized = InstallSecretsRequest {
        session_id: "session-oversized".into(),
        generation: "generation-1".into(),
        env_names: vec!["A".into()],
        values: HashMap::from([("A".into(), oversized_value)]),
    };
    assert_eq!(
        serde_jcs::to_vec(&oversized.values).unwrap().len(),
        brain_protocol::MAX_SESSION_SECRET_DOCUMENT_BYTES + 1
    );
    assert_eq!(
        oversized_environment
            .install_secrets(oversized)
            .await
            .unwrap_err()
            .code,
        EnvironmentErrorCode::InvalidRequest
    );
}

#[tokio::test]
async fn undeclared_secret_names_are_refused_without_installing_values() {
    let (_directory, environment, _digest) = prepared_environment().await;
    let error = environment
        .install_secrets(InstallSecretsRequest {
            session_id: "session-1".into(),
            generation: "generation-1".into(),
            env_names: vec!["FIXTURE_SECRET".into()],
            values: HashMap::from([("NOT_DECLARED".into(), "secret".into())]),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, EnvironmentErrorCode::InvalidRequest);
    assert!(environment.artifacts.secrets.read().await.is_empty());
}

#[test]
fn customer_environment_cannot_replace_runtime_or_connector_authority() {
    for name in [
        "LD_PRELOAD",
        "node_options",
        "HTTPS_PROXY",
        "AEX_WORKSPACE",
        "ENVIRONMENT_TOOL_RUNNER",
        "OPENSSL_MODULES",
    ] {
        assert!(reserved_tool_environment(name), "{name}");
    }
    for name in ["OPENAI_API_KEY", "PROC_SECRET", "DATABASE_URL"] {
        assert!(!reserved_tool_environment(name), "{name}");
    }
}

#[cfg(unix)]
fn sandbox_request(
    execution_id: &str,
    command: &str,
    interactive: bool,
) -> SandboxExecutionRequest {
    let mut request: SandboxExecutionRequest = serde_json::from_value(serde_json::json!({
        "execution_id": execution_id,
        "expected_generation": "generation-1",
        "input": {
            "command": command,
            "cwd": "/workspace",
            "interactive": interactive
        },
        "network": {"kind": "none"},
        "request_digest": "0".repeat(64),
        "resources": {
            "max_output_bytes": 65536,
            "timeout_ms": 5000
        },
        "target": {
            "binding_ref": "sandbox-binding-1",
            "kind": "environment",
            "root_id": "root-1",
            "session_id": "session-1"
        }
    }))
    .unwrap();
    request.request_digest = sandbox_execution_request_digest(&request);
    request
}

#[cfg(unix)]
async fn wait_terminal(environment: &Environment, operation: OperationRef) -> OperationObservation {
    environment
        .observe(ObserveRequest {
            cursor: "0".parse().unwrap(),
            operation,
            wait_ms: 5_000,
        })
        .await
        .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn sandbox_exact_replay_and_conflicting_digest_never_repeat_the_effect() {
    let (_directory, environment, _digest) = prepared_environment().await;
    let first = sandbox_request("sandbox-execution-1", "printf first >> effect.txt", false);
    let receipt = environment.execute_sandbox(first.clone()).await.unwrap();
    assert_eq!(
        wait_terminal(&environment, receipt.operation.clone())
            .await
            .state,
        ContractOperationState::Terminal
    );

    let replay = environment.execute_sandbox(first).await.unwrap();
    assert!(replay.replayed);
    let conflict = environment
        .execute_sandbox(sandbox_request(
            "sandbox-execution-1",
            "printf second >> effect.txt",
            false,
        ))
        .await
        .unwrap_err();
    assert_eq!(conflict.code, EnvironmentErrorCode::OperationConflict);
    assert_eq!(
        std::fs::read_to_string(environment.cfg.workspace.join("effect.txt")).unwrap(),
        "first"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_ack_replays_and_permanently_fences_resubmission() {
    let (_directory, environment, _digest) = prepared_environment().await;
    let request = sandbox_request(
        "sandbox-execution-acked",
        "printf once >> acked-effect.txt",
        false,
    );
    let receipt = environment.execute_sandbox(request.clone()).await.unwrap();
    let terminal = wait_terminal(&environment, receipt.operation.clone())
        .await
        .terminal
        .expect("terminal result");
    let acknowledgement = AcknowledgeTerminalRequest {
        operation: receipt.operation.clone(),
        terminal_digest: terminal.terminal_digest,
    };
    assert!(
        environment
            .acknowledge_terminal(acknowledgement.clone())
            .await
            .unwrap()
            .acknowledged
    );
    assert!(
        environment
            .acknowledge_terminal(acknowledgement)
            .await
            .unwrap()
            .acknowledged
    );

    let exact = environment.execute_sandbox(request).await.unwrap_err();
    assert_eq!(exact.code, EnvironmentErrorCode::OperationUnknown);
    let conflicting = environment
        .execute_sandbox(sandbox_request(
            "sandbox-execution-acked",
            "printf twice >> acked-effect.txt",
            false,
        ))
        .await
        .unwrap_err();
    assert_eq!(conflicting.code, EnvironmentErrorCode::OperationConflict);
    assert_eq!(
        std::fs::read_to_string(environment.cfg.workspace.join("acked-effect.txt")).unwrap(),
        "once"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn write_stdin_is_exact_pair_idempotent() {
    let (_directory, environment, _digest) = prepared_environment().await;
    let execution = sandbox_request(
        "sandbox-execution-stdin",
        "IFS= read -r line; printf '%s' \"$line\" > stdin.txt",
        true,
    );
    let receipt = environment
        .execute_sandbox(execution.clone())
        .await
        .unwrap();
    let mut write: WriteStdinRequest = serde_json::from_value(serde_json::json!({
        "execution_id": execution.execution_id.clone(),
        "expected_generation": "generation-1",
        "eof": false,
        "operation_id": "stdin-write-1",
        "request_digest": "0".repeat(64),
        "target": execution.target.clone(),
        "text": "hello\n"
    }))
    .unwrap();
    write.request_digest = write_stdin_request_digest(&write);
    // JSON Schema counts Unicode scalar values, while Linux PIPE_BUF is bytes. The runtime
    // closes that gap before reserving the idempotency key or touching the pipe.
    let mut oversized = write.clone();
    oversized.operation_id = "stdin-write-oversized".parse().unwrap();
    oversized.text = "é"
        .repeat(brain_protocol::MAX_WRITE_STDIN_BYTES)
        .parse()
        .unwrap();
    oversized.request_digest = write_stdin_request_digest(&oversized);
    let error = environment.write_stdin(oversized).await.unwrap_err();
    assert_eq!(error.code, EnvironmentErrorCode::InvalidRequest);

    let first = environment.write_stdin(write.clone()).await.unwrap();
    assert!(first.accepted);
    assert!(!first.replayed);
    assert!(canonical_equal(&first.observation.operation, &receipt.operation).unwrap());
    let replay = environment.write_stdin(write.clone()).await.unwrap();
    assert!(replay.accepted);
    assert!(replay.replayed);
    assert!(canonical_equal(&replay.observation.operation, &receipt.operation).unwrap());
    let mut conflict = write;
    conflict.text = "different\n".parse().unwrap();
    conflict.request_digest = write_stdin_request_digest(&conflict);
    let error = environment.write_stdin(conflict).await.unwrap_err();
    assert_eq!(error.code, EnvironmentErrorCode::OperationConflict);
    assert_eq!(
        wait_terminal(&environment, receipt.operation).await.state,
        ContractOperationState::Terminal
    );
    assert_eq!(
        std::fs::read_to_string(environment.cfg.workspace.join("stdin.txt")).unwrap(),
        "hello"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn write_stdin_supports_explicit_eof_and_observation_only_poll() {
    let (_directory, environment, _digest) = prepared_environment().await;
    let execution = sandbox_request("sandbox-execution-eof", "cat > stdin-eof.txt", true);
    let submitted = environment
        .execute_sandbox(execution.clone())
        .await
        .unwrap();
    let mut close: WriteStdinRequest = serde_json::from_value(serde_json::json!({
        "eof": true,
        "execution_id": execution.execution_id.clone(),
        "expected_generation": "generation-1",
        "operation_id": "stdin-close-1",
        "request_digest": "0".repeat(64),
        "target": execution.target.clone(),
        "text": "without-newline"
    }))
    .unwrap();
    close.request_digest = write_stdin_request_digest(&close);
    let first = environment.write_stdin(close.clone()).await.unwrap();
    assert!(first.accepted);
    assert!(!first.replayed);
    assert!(canonical_equal(&first.observation.operation, &submitted.operation).unwrap());

    let terminal = wait_terminal(&environment, submitted.operation.clone()).await;
    assert_eq!(terminal.state, ContractOperationState::Terminal);
    assert_eq!(
        std::fs::read_to_string(environment.cfg.workspace.join("stdin-eof.txt")).unwrap(),
        "without-newline"
    );

    let replay = environment.write_stdin(close).await.unwrap();
    assert!(replay.accepted);
    assert!(replay.replayed);
    assert_eq!(replay.observation.state, ContractOperationState::Terminal);

    let mut poll: WriteStdinRequest = serde_json::from_value(serde_json::json!({
        "eof": false,
        "execution_id": execution.execution_id,
        "expected_generation": "generation-1",
        "operation_id": "stdin-poll-1",
        "request_digest": "0".repeat(64),
        "target": execution.target,
        "text": ""
    }))
    .unwrap();
    poll.request_digest = write_stdin_request_digest(&poll);
    let polled = environment.write_stdin(poll).await.unwrap();
    assert!(!polled.accepted);
    assert_eq!(polled.observation.state, ContractOperationState::Terminal);
    assert_eq!(environment.stdin.book.lock().await.records.len(), 2);
    environment
        .acknowledge_terminal(AcknowledgeTerminalRequest {
            operation: submitted.operation,
            terminal_digest: terminal.terminal.unwrap().terminal_digest,
        })
        .await
        .unwrap();
    assert!(environment.stdin.book.lock().await.records.is_empty());
}
