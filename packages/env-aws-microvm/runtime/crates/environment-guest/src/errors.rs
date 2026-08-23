use axum::http::StatusCode;

use crate::acks::AckStoreError;
use crate::file_effects::FileEffectStoreError;
use brain_protocol::environment::{EnvironmentError, EnvironmentErrorCode};
use environment_core::operation::OperationError;

/// HTTP projection of a Environment error for the install routes. The code and `retryable` flag carry
/// the real distinction; the status keeps plain HTTP clients from reading every failure as 409.
pub fn status_for(code: EnvironmentErrorCode) -> StatusCode {
    match code {
        EnvironmentErrorCode::InvalidRequest => StatusCode::BAD_REQUEST,
        EnvironmentErrorCode::FileNotFound | EnvironmentErrorCode::OperationUnknown => StatusCode::NOT_FOUND,
        EnvironmentErrorCode::BindingConflict
        | EnvironmentErrorCode::OperationConflict
        | EnvironmentErrorCode::GenerationConflict
        | EnvironmentErrorCode::SandboxNotMaterialized => StatusCode::CONFLICT,
        EnvironmentErrorCode::SandboxGone => StatusCode::GONE,
        EnvironmentErrorCode::ResourceExhausted => StatusCode::PAYLOAD_TOO_LARGE,
        EnvironmentErrorCode::CapabilityUnavailable | EnvironmentErrorCode::TemporarilyUnavailable => {
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

pub fn environment_error(code: EnvironmentErrorCode, retryable: bool, message: impl Into<String>) -> EnvironmentError {
    let mut message = message.into();
    if message.is_empty() {
        message = "Environment request failed".into();
    }
    truncate_utf8(&mut message, 4096);
    EnvironmentError {
        code,
        details: serde_json::Map::new(),
        message: message
            .parse()
            .unwrap_or_else(|_| "Environment request failed".parse().expect("bounded message")),
        retryable,
    }
}

pub(crate) fn truncate_utf8(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

pub fn invalid(message: impl Into<String>) -> EnvironmentError {
    environment_error(EnvironmentErrorCode::InvalidRequest, false, message)
}

pub fn unavailable(message: impl Into<String>) -> EnvironmentError {
    environment_error(EnvironmentErrorCode::TemporarilyUnavailable, true, message)
}

pub(crate) fn operation_error(error: OperationError) -> EnvironmentError {
    let code = match error {
        OperationError::IdempotencyConflict | OperationError::TerminalConflict => {
            EnvironmentErrorCode::OperationConflict
        }
        OperationError::Unknown => EnvironmentErrorCode::OperationUnknown,
        OperationError::Capacity | OperationError::TerminalCapacity => {
            EnvironmentErrorCode::ResourceExhausted
        }
        OperationError::InvalidIdentity(_)
        | OperationError::AlreadyTerminal
        | OperationError::NotTerminal
        | OperationError::TerminalDigestMismatch => EnvironmentErrorCode::InvalidRequest,
    };
    environment_error(code, false, error.to_string())
}

pub(crate) fn stdin_conflict() -> EnvironmentError {
    environment_error(
        EnvironmentErrorCode::OperationConflict,
        false,
        "stdin operation_id is already reserved for a different request digest",
    )
}

pub(crate) fn ack_store_error(error: AckStoreError) -> EnvironmentError {
    let (code, retryable) = match error {
        AckStoreError::Conflict => (EnvironmentErrorCode::OperationConflict, false),
        AckStoreError::Capacity => (EnvironmentErrorCode::ResourceExhausted, false),
        AckStoreError::Invalid(_) => (EnvironmentErrorCode::InvalidRequest, false),
        AckStoreError::Io(_) => (EnvironmentErrorCode::TemporarilyUnavailable, true),
        AckStoreError::Corrupt(_) => (EnvironmentErrorCode::TemporarilyUnavailable, false),
    };
    environment_error(code, retryable, error.to_string())
}

pub(crate) fn file_effect_store_error(error: FileEffectStoreError) -> EnvironmentError {
    let (code, retryable) = match error {
        FileEffectStoreError::Conflict => (EnvironmentErrorCode::BindingConflict, false),
        FileEffectStoreError::Ambiguous => (EnvironmentErrorCode::OperationUnknown, false),
        FileEffectStoreError::Capacity => (EnvironmentErrorCode::ResourceExhausted, false),
        FileEffectStoreError::Invalid(_) => (EnvironmentErrorCode::InvalidRequest, false),
        FileEffectStoreError::Io(_) => (EnvironmentErrorCode::TemporarilyUnavailable, true),
        FileEffectStoreError::Corrupt(_) => (EnvironmentErrorCode::CapabilityUnavailable, false),
    };
    environment_error(code, retryable, error.to_string())
}

pub(crate) fn generation_conflict() -> EnvironmentError {
    environment_error(
        EnvironmentErrorCode::GenerationConflict,
        false,
        "request does not match the live physical generation",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_unicode_diagnostics_truncate_only_on_a_character_boundary() {
        let error = invalid("x".repeat(4095) + "🦀tail");
        assert!(error.message.as_str().len() <= 4096);
        assert!(error.message.as_str().ends_with('x'));
    }
}
