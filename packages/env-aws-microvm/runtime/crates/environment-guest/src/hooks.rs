//! Provider launch/build hooks.
//!
//! `/run` is the only armed lifecycle mutation. Resume, suspend, and terminate hooks remain absent:
//! workspace durability is explicit, and an unauthenticated in-guest lifecycle endpoint would let
//! hostile Tool code mutate supervisor state.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use environment_wire::{RunEnvelope, RunPayload};
use serde_json::{Value, json};

use crate::environment::Environment;

pub const HOOK_PREFIX: &str = "/aws/lambda-microvms/runtime/v1";

pub async fn run(State(environment): State<Arc<Environment>>, body: String) -> (StatusCode, Json<Value>) {
    let envelope: RunEnvelope = match serde_json::from_str(&body) {
        Ok(envelope) => envelope,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "malformed provider run envelope"})),
            );
        }
    };
    let payload: RunPayload = match serde_json::from_str(&envelope.run_hook_payload) {
        Ok(payload) => payload,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "malformed Environment run payload"})),
            );
        }
    };
    // Every real caller sends microvmId; inventing a target from the generation would
    // surface later as baffling GenerationConflicts far from this hook. Refuse at the door.
    let Some(target_ref) = envelope.microvm_id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "the run envelope is missing microvmId"})),
        );
    };
    match environment.arm(target_ref, payload).await {
        Ok(replayed) => (StatusCode::OK, Json(json!({"replayed": replayed}))),
        Err(error) => (
            StatusCode::CONFLICT,
            Json(json!({"error": error.message.as_str(), "code": error.code})),
        ),
    }
}

/// Build-only rootfs contract. Once armed it intentionally disappears.
pub async fn ready(State(environment): State<Arc<Environment>>) -> (StatusCode, Json<Value>) {
    if environment.armed().await {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "not found"})));
    }
    let mut failures = Vec::new();
    for (name, directory) in [
        ("workspace", &environment.cfg.workspace),
        ("state", &environment.cfg.state_dir),
        ("tools", &environment.cfg.tool_dir),
        ("objects", &environment.cfg.object_dir),
    ] {
        if !directory.is_dir() {
            failures.push(format!("{name} is not a directory"));
            continue;
        }
        let probe = directory.join(".environment-ready-probe");
        match std::fs::write(&probe, b"ok") {
            Ok(()) => {
                let _ = std::fs::remove_file(probe);
            }
            Err(error) => failures.push(format!("{name} is not writable: {error}")),
        }
    }
    if failures.is_empty() {
        (StatusCode::OK, Json(json!({"ok": true})))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"ok": false, "failures": failures})),
        )
    }
}

/// Build-only curated toolchain probe. It does not validate customer bundles.
pub async fn validate(State(environment): State<Arc<Environment>>) -> (StatusCode, Json<Value>) {
    if environment.armed().await {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "not found"})));
    }
    let tools: &[(&str, &[&str])] = &[
        ("bash", &["--version"]),
        ("python3", &["--version"]),
        ("node", &["--version"]),
        ("git", &["--version"]),
        ("rg", &["--version"]),
    ];
    let mut failures = Vec::new();
    for (tool, args) in tools {
        match tokio::time::timeout(
            std::time::Duration::from_secs(20),
            tokio::process::Command::new(tool).args(*args).output(),
        )
        .await
        {
            Ok(Ok(output)) if output.status.success() => {}
            Ok(Ok(output)) => failures.push(format!("{tool}: exit {:?}", output.status.code())),
            Ok(Err(error)) => failures.push(format!("{tool}: {error}")),
            Err(_) => failures.push(format!("{tool}: timed out")),
        }
    }
    if failures.is_empty() {
        (StatusCode::OK, Json(json!({"ok": true})))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"ok": false, "failures": failures})),
        )
    }
}

#[cfg(test)]
mod tests {
    use environment_wire::{RunEnvelope, RunPayload};

    #[test]
    fn provider_envelope_carries_a_closed_cloud_credential_free_payload() {
        let body = r#"{"microvmId":"mvm-abc","runHookPayload":"{\"contract_digest\":\"d\",\"generation\":\"g\",\"expires_at_ms\":1,\"root_id\":\"r\",\"owner_session_id\":\"s\",\"connector\":\"none\",\"resource_class\":\"small\",\"resources\":{\"max_output_bytes\":1,\"timeout_ms\":1},\"network\":{\"kind\":\"none\"},\"control_token\":\"control-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}"}"#;
        let envelope: RunEnvelope = serde_json::from_str(body).expect("provider envelope");
        assert_eq!(envelope.microvm_id.as_deref(), Some("mvm-abc"));
        assert!(serde_json::from_str::<RunPayload>(&envelope.run_hook_payload).is_ok());
        assert!(!envelope.run_hook_payload.contains("auth_token"));
        assert!(!envelope.run_hook_payload.contains("access_key"));
    }
}
