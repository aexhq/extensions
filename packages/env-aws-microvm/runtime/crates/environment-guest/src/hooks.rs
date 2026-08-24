//! Provider launch/build hooks.
//!
//! `/run` is the only armed lifecycle mutation. Resume, suspend, and terminate hooks remain absent:
//! workspace durability is explicit, and an unauthenticated in-guest lifecycle endpoint would let
//! hostile Tool code mutate supervisor state.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse as _, Response};
use environment_wire::{RunEnvelope, RunPayload};
use serde_json::{Value, json};

use crate::environment::Environment;

pub const HOOK_PREFIX: &str = "/aws/lambda-microvms/runtime/v1";

pub async fn run(State(environment): State<Arc<Environment>>, body: String) -> Response {
    let envelope: RunEnvelope = match serde_json::from_str(&body) {
        Ok(envelope) => envelope,
        Err(_) => {
            return run_response(
                StatusCode::BAD_REQUEST,
                json!({"error": "malformed provider run envelope"}),
            );
        }
    };
    let payload: RunPayload = match serde_json::from_str(&envelope.run_hook_payload) {
        Ok(payload) => payload,
        Err(_) => {
            return run_response(
                StatusCode::BAD_REQUEST,
                json!({"error": "malformed Environment run payload"}),
            );
        }
    };
    // Every real caller sends microvmId; inventing a target from the generation would
    // surface later as baffling GenerationConflicts far from this hook. Refuse at the door.
    let Some(target_ref) = envelope.microvm_id else {
        return run_response(
            StatusCode::BAD_REQUEST,
            json!({"error": "the run envelope is missing microvmId"}),
        );
    };
    match environment.arm(target_ref, payload).await {
        Ok(replayed) => run_response(StatusCode::OK, json!({"replayed": replayed})),
        Err(error) => run_response(
            StatusCode::CONFLICT,
            json!({"error": error.message.as_str(), "code": error.code}),
        ),
    }
}

fn run_response(status: StatusCode, body: Value) -> Response {
    // The provider owns this one lifecycle request. Closing it with the response prevents its
    // 60-second hook deadline from remaining attached to an already-armed MicroVM.
    (status, [(header::CONNECTION, "close")], Json(body)).into_response()
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
    use axum::http::{StatusCode, header};
    use environment_wire::{RunEnvelope, RunPayload};
    use serde_json::json;

    use super::run_response;

    #[test]
    fn run_hook_response_closes_the_provider_connection() {
        let response = run_response(StatusCode::OK, json!({"ok": true}));
        assert_eq!(
            response.headers().get(header::CONNECTION),
            Some(&axum::http::HeaderValue::from_static("close"))
        );
    }

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
