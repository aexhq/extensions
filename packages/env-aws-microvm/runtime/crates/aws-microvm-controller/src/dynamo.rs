//! Shared DynamoDB attribute and error helpers for the registry and definition stores.

use aws_sdk_dynamodb::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_dynamodb::types::AttributeValue;

pub(crate) fn s(value: impl Into<String>) -> AttributeValue {
    AttributeValue::S(value.into())
}

pub(crate) fn n(value: u64) -> AttributeValue {
    AttributeValue::N(value.to_string())
}

pub(crate) fn conditional_failure<E: ProvideErrorMetadata, R>(error: &SdkError<E, R>) -> bool {
    matches!(
        error,
        SdkError::ServiceError(service)
            if service.err().code() == Some("ConditionalCheckFailedException")
    )
}

/// `"{operation}: {code}: {message}"` — internal storage diagnostics only; callers must not
/// forward this text through the public Environment contract.
pub(crate) fn storage_detail<E: ProvideErrorMetadata, R>(
    operation: &str,
    error: &SdkError<E, R>,
) -> String {
    let description = match error {
        SdkError::ServiceError(service) => format!(
            "{}: {}",
            service.err().code().unwrap_or("service error"),
            service.err().message().unwrap_or("")
        ),
        other => other.to_string(),
    };
    format!("{operation}: {description}")
}
