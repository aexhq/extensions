//! Canonical identities for the private provider protocol.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::{
    Digest, Identifier, OperationEnvelope, SandboxCopyRequest, SandboxCopyRequestDirection,
    SandboxExecutionRequest, SandboxFileWriteRequest, TerminalResult, WriteStdinRequest,
};

pub const ENVIRONMENT_CONTRACT_DIGEST: &str =
    "7aea6ad07f67b322300c752017bf2c5cda692e0e4fc6579fe6af5a7f7ab606dc";

pub fn canonical_digest<T: Serialize>(value: &T) -> Result<Digest, serde_json::Error> {
    let canonical = serde_jcs::to_vec(value)?;
    Ok(hex::encode(Sha256::digest(canonical))
        .parse()
        .expect("SHA-256 hex satisfies the provider Digest schema"))
}

pub fn environment_binding_ref(root_id: &str, environment_name: &str) -> Identifier {
    let digest = hex::encode(Sha256::digest(
        format!("brain.environment-target\0{root_id}\0{environment_name}").as_bytes(),
    ));
    format!("bnd_{}", &digest[..24])
        .parse()
        .expect("derived environment binding ref satisfies Identifier")
}

pub fn operation_request_digest(envelope: &OperationEnvelope) -> Digest {
    let mut value = serde_json::to_value(envelope).expect("an operation envelope serializes");
    let object = value
        .as_object_mut()
        .expect("an operation envelope is a JSON object");
    object.remove("request_digest");
    object.remove("trace");
    canonical_digest(&value).expect("an operation envelope is canonicalizable")
}

pub fn sandbox_execution_request_digest(request: &SandboxExecutionRequest) -> Digest {
    request_digest_without_self(request)
}

pub fn write_stdin_request_digest(request: &WriteStdinRequest) -> Digest {
    request_digest_without_self(request)
}

pub fn sandbox_file_write_request_digest(request: &SandboxFileWriteRequest) -> Digest {
    let mut value = request_value_without_self(request);
    if let Some(authority) = value
        .get_mut("source")
        .and_then(Value::as_object_mut)
        .and_then(|source| source.get_mut("fetch"))
    {
        remove_ephemeral_authority(authority, true);
    }
    canonical_digest(&value).expect("a sandbox file write request is canonicalizable")
}

pub fn sandbox_copy_request_digest(request: &SandboxCopyRequest) -> Digest {
    let mut value = request_value_without_self(request);
    if let Some(authority) = value.get_mut("transfer") {
        remove_ephemeral_authority(
            authority,
            request.direction == SandboxCopyRequestDirection::Import,
        );
    }
    canonical_digest(&value).expect("a sandbox copy request is canonicalizable")
}

fn request_digest_without_self<T: Serialize>(request: &T) -> Digest {
    canonical_digest(&request_value_without_self(request)).expect("a request is canonicalizable")
}

fn request_value_without_self<T: Serialize>(request: &T) -> Value {
    let mut value = serde_json::to_value(request).expect("a request serializes");
    value
        .as_object_mut()
        .expect("a request is a JSON object")
        .remove("request_digest");
    value
}

fn remove_ephemeral_authority(value: &mut Value, remove_transfer_id: bool) {
    if let Some(authority) = value.as_object_mut() {
        authority.remove("url");
        authority.remove("headers");
        authority.remove("expires_at_ms");
        if remove_transfer_id {
            authority.remove("transfer_id");
        }
    }
}

pub fn terminal_result_digest(terminal: &TerminalResult) -> Digest {
    let mut value = serde_json::to_value(terminal).expect("a terminal result serializes");
    value
        .as_object_mut()
        .expect("a terminal result is a JSON object")
        .remove("terminal_digest");
    canonical_digest(&value).expect("a terminal result is canonicalizable")
}

pub fn terminal_inline_bytes(value: &Value) -> Result<usize, serde_json::Error> {
    Ok(serde_jcs::to_vec(value)?.len())
}

pub fn terminal_inline_fits(value: &Value) -> bool {
    terminal_inline_bytes(value).is_ok_and(|bytes| bytes <= crate::MAX_TOOL_TERMINAL_INLINE_BYTES)
}
