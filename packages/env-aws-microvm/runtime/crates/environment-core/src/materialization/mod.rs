//! Durable target materialization before any guest effect is allowed to start.
//!
//! Brain has already committed the operation identity and digest when this state machine runs.
//! Environment only needs one durable routing record per physical target: after the record is installed,
//! retries reach the same guest and that guest atomically deduplicates `(operation_id, digest)`.
//! There is deliberately no per-operation database write on the ordinary execution path.

use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::connector::ConnectorClass;

pub const TARGET_KEY_PREFIX: &str = "target:";
pub const ENVIRONMENT_TARGET_PREFIX: &str = "target:environment:";
pub use crate::page::MAX_PAGE as MAX_TARGET_PAGE;
/// A durable uncertainty lease can span the provider's full target lifetime, but callers waiting
/// on the worker that owns a normal launch must poll on a short bounded cadence. Exposing the
/// lease deadline as `retry_after_ms` would turn an ordinary first-call race into an eight-hour
/// outage even though the installed target is normally visible within seconds.
pub const MAX_MATERIALIZATION_POLL_MS: u64 = 1_000;
pub const MAX_REASON_BYTES: usize = 512;

// The secret newtypes live in the vocabulary crate; this re-export keeps every consumer of the
// materialization contract on one import path.
pub use environment_policy::secret::{
    ControlToken, DurableLaunchRequest, MAX_DURABLE_LAUNCH_REQUEST_BYTES, SecretError,
};

impl From<SecretError> for MaterializationError {
    fn from(error: SecretError) -> Self {
        match error {
            SecretError::InvalidControlToken => MaterializationError::InvalidControlToken,
            SecretError::InvalidLaunchRequest => MaterializationError::InvalidLaunchRequest,
        }
    }
}

/// A logical target within one root session tree.
mod materializer;
mod port;
mod record;
mod spec;

pub use materializer::*;
pub use port::*;
pub use record::*;
pub use spec::*;

#[cfg(any(test, feature = "test-support"))]
mod memory;
#[cfg(any(test, feature = "test-support"))]
pub use memory::*;

fn validate_identifier(value: &str, field: &'static str) -> Result<(), MaterializationError> {
    environment_policy::identity::validate_identifier(value, field)
        .map_err(|error| MaterializationError::InvalidIdentity(error.field))
}

fn validate_bounded_token(
    value: &str,
    field: &'static str,
    max: usize,
) -> Result<(), MaterializationError> {
    environment_policy::identity::validate_bounded_token(value, field, max)
        .map_err(|error| MaterializationError::InvalidIdentity(error.field))
}

fn validate_digest(value: &str, field: &'static str) -> Result<(), MaterializationError> {
    environment_policy::identity::validate_digest(value, field)
        .map_err(|error| MaterializationError::InvalidIdentity(error.field))
}

fn validate_reason(reason: &str) -> Result<(), MaterializationError> {
    if reason.is_empty() || reason.len() > MAX_REASON_BYTES {
        return Err(MaterializationError::InvalidIdentity("reason"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MaterializationError {
    #[error("{0} does not satisfy the canonical Environment identifier grammar")]
    InvalidIdentity(&'static str),
    #[error("materialization lease must have a positive, representable duration")]
    InvalidLease,
    #[error("durable provider launch request is empty or exceeds its sealed byte bound")]
    InvalidLaunchRequest,
    #[error("generation control token is outside its exact secret boundary")]
    InvalidControlToken,
    #[error("only an environment target may be replaced after confirmed loss")]
    InvalidReplacement,
    #[error("target materialized memory must be a positive bounded MiB value")]
    InvalidCapacity,
    #[error("target is sealed to a different connector, image, resource, or network policy")]
    SpecConflict,
    #[error("target materialization is in progress; retry after {retry_after_ms} ms")]
    Pending { retry_after_ms: u64 },
    #[error("target generation is gone")]
    Gone,
    #[error("target was explicitly terminated")]
    Terminated,
    #[error("provider capacity exhausted ({scope}); retry after {retry_after_ms} ms: {message}")]
    Capacity {
        scope: String,
        retry_after_ms: u64,
        message: String,
    },
    #[error("launch was rejected before target creation: {0}")]
    LaunchRejected(String),
    #[error("launch dependency failed before target creation: {0}")]
    LaunchRetryable(String),
    #[error("launch outcome is unknown; the lease is retained: {0}")]
    LaunchOutcomeUnknown(String),
    #[error("materialization reservation was lost; stale-target cleanup: {cleanup:?}")]
    ReservationLost { cleanup: Option<String> },
    #[error("durable target registry unavailable: {0}")]
    Storage(String),
    #[error("durable target registry contains an invalid record: {0}")]
    Corrupt(String),
}

#[cfg(test)]
mod tests;
