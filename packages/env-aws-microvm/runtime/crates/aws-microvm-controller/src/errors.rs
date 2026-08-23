//! EnvironmentError constructors and storage/materialization error classification.

use crate::*;

/// One shape for every in-process admission-budget refusal.
fn capacity_error(
    scope: &'static str,
    message: &'static str,
    limits: &[(&'static str, u64)],
) -> EnvironmentError {
    let mut value = error(EnvironmentErrorCode::ResourceExhausted, true, message);
    value.details.insert("scope".into(), scope.into());
    for (key, amount) in limits {
        value.details.insert((*key).into(), (*amount).into());
    }
    value
}

pub(crate) fn preparation_cache_capacity_error(limit_bytes: usize) -> EnvironmentError {
    capacity_error(
        "environment_preparation_cache_bytes",
        "the in-process session preparation metadata budget is full",
        &[
            ("limit_bytes", limit_bytes as u64),
            ("entry_limit", MAX_CACHED_PREPARATIONS as u64),
        ],
    )
}

pub(crate) fn bundle_cache_capacity_error(limit_bytes: usize) -> EnvironmentError {
    capacity_error(
        "environment_bundle_cache_bytes",
        "the in-process verified bundle memory budget is full",
        &[("limit_bytes", limit_bytes as u64)],
    )
}

pub(crate) fn bundle_fetch_capacity_error(limit_bytes: usize) -> EnvironmentError {
    capacity_error(
        "environment_bundle_fetch_bytes",
        "the in-process cold bundle fetch budget is full",
        &[("limit_bytes", limit_bytes as u64)],
    )
}

pub(crate) fn bundle_cache_entry_capacity_error() -> EnvironmentError {
    capacity_error(
        "environment_bundle_cache_entries",
        "the in-process verified bundle entry budget is full",
        &[("limit", MAX_CACHED_BUNDLES as u64)],
    )
}

/// Sanitized retryable failure that still leaves an operator trail. The cause is logged here
/// precisely because it must never enter the public Environment contract.
pub(crate) fn temporary_from(
    message: &'static str,
    cause: impl std::fmt::Display,
) -> EnvironmentError {
    tracing::warn!(%cause, "{message}");
    temporary(message)
}

pub(crate) fn invalid(message: impl Into<String>) -> EnvironmentError {
    error(EnvironmentErrorCode::InvalidRequest, false, message)
}

/// A reply variant that does not match the request method is a host/guest contract violation
/// (for example protocol-version skew), never a transient fault: a retry replays the exact same
/// mismatch, so fail fast and non-retryable.
pub(crate) fn wrong_reply(context: &'static str) -> EnvironmentError {
    error(
        EnvironmentErrorCode::InvalidRequest,
        false,
        format!("guest returned the wrong {context} reply"),
    )
}

pub(crate) fn binding_error(message: impl Into<String>) -> EnvironmentError {
    error(EnvironmentErrorCode::BindingConflict, false, message)
}

pub(crate) fn generation_error() -> EnvironmentError {
    error(
        EnvironmentErrorCode::GenerationConflict,
        false,
        "request does not match the live sandbox generation",
    )
}

/// Once the operation submit RPC has been attempted, loss of its physical generation cannot prove
/// that the guest effect did not start. Brain has durable intent but may not yet have received the
/// operation receipt, so returning `sandbox_gone` would let recovery route the target-less intent
/// into a replacement generation. Preserve the uncertainty explicitly and never repeat the effect.
pub(crate) fn classify_submit_delivery_error(error_value: EnvironmentError) -> EnvironmentError {
    if error_value.code == EnvironmentErrorCode::SandboxGone {
        error(
            EnvironmentErrorCode::OperationUnknown,
            false,
            "managed operation delivery became unknown when its physical generation was lost",
        )
    } else {
        error_value
    }
}

// Both mappers enumerate their closed source enums exhaustively: a new variant must choose its
// public classification at compile time instead of silently becoming a non-retryable
// InvalidRequest.
pub(crate) fn definition_error(error_value: DefinitionError) -> EnvironmentError {
    match error_value {
        DefinitionError::Conflict => binding_error(error_value.to_string()),
        DefinitionError::Storage(_) => temporary_from(
            "definition registry is temporarily unavailable",
            error_value,
        ),
        DefinitionError::InvalidIdentity(_)
        | DefinitionError::InvalidPayload(_)
        | DefinitionError::PayloadTooLarge
        | DefinitionError::InvalidLimit
        | DefinitionError::Corrupt(_) => invalid(error_value.to_string()),
    }
}

pub(crate) fn root_seal_error(error_value: DefinitionError) -> EnvironmentError {
    if error_value == DefinitionError::Conflict {
        error(
            EnvironmentErrorCode::GenerationConflict,
            false,
            "root sandbox network/resource seal conflicts with an earlier preparation",
        )
    } else {
        definition_error(error_value)
    }
}

pub(crate) fn materialization_error(error_value: MaterializationError) -> EnvironmentError {
    match error_value {
        MaterializationError::Capacity {
            scope,
            retry_after_ms,
            message,
        } => {
            let mut value = error(EnvironmentErrorCode::ResourceExhausted, true, message);
            value.details.insert("scope".into(), scope.into());
            value
                .details
                .insert("retry_after_ms".into(), retry_after_ms.into());
            value
        }
        MaterializationError::Pending { retry_after_ms } => {
            let mut value = temporary(error_value.to_string());
            value
                .details
                .insert("retry_after_ms".into(), retry_after_ms.into());
            value
        }
        MaterializationError::Gone | MaterializationError::Terminated => error(
            EnvironmentErrorCode::SandboxGone,
            false,
            error_value.to_string(),
        ),
        MaterializationError::SpecConflict => error(
            EnvironmentErrorCode::GenerationConflict,
            false,
            error_value.to_string(),
        ),
        MaterializationError::Storage(_) => {
            temporary_from("target registry is temporarily unavailable", error_value)
        }
        MaterializationError::LaunchRetryable(_) => temporary_from(
            "sandbox launch dependency is temporarily unavailable",
            error_value,
        ),
        MaterializationError::LaunchOutcomeUnknown(_) => temporary_from(
            "sandbox launch outcome is unknown; bounded recovery will reconcile",
            error_value,
        ),
        MaterializationError::ReservationLost { .. } => temporary_from(
            "target reservation was superseded by a concurrent transition",
            error_value,
        ),
        MaterializationError::LaunchRejected(_) => error(
            EnvironmentErrorCode::CapabilityUnavailable,
            false,
            error_value.to_string(),
        ),
        MaterializationError::InvalidIdentity(_)
        | MaterializationError::InvalidLease
        | MaterializationError::InvalidLaunchRequest
        | MaterializationError::InvalidControlToken
        | MaterializationError::InvalidReplacement
        | MaterializationError::InvalidCapacity
        | MaterializationError::Corrupt(_) => invalid(error_value.to_string()),
    }
}
