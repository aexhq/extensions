//! Receipt projection and pure request validation.

use super::*;

pub(crate) fn operation_ref(
    envelope: &OperationEnvelope,
    physical: &TargetSnapshot,
) -> Result<OperationRef, EnvironmentError> {
    Ok(OperationRef {
        generation: physical
            .generation
            .as_str()
            .parse()
            .map_err(|_| invalid("generation is not a canonical operation locator"))?,
        operation_id: envelope.operation_id.clone(),
        receipt_ref: operation_receipt_ref(
            envelope.operation_id.as_str(),
            envelope.request_digest.as_str(),
            physical.target_ref.as_str(),
            physical.generation.as_str(),
        )?,
        request_digest: envelope.request_digest.clone(),
        target: SandboxTarget {
            binding_ref: envelope.binding_ref.clone(),
            kind: TargetKind::Default,
            root_id: envelope.root_id.clone(),
            sandbox_id: None,
            session_id: envelope.session_id.clone(),
        },
        target_ref: physical
            .target_ref
            .as_str()
            .parse()
            .map_err(|_| invalid("target_ref is not a canonical operation locator"))?,
    })
}

/// The target reference routes later work to one physical filesystem; the receipt reference names
/// one reserved operation on that target. It is deterministic so a lost submit response can be
/// reconstructed without adding a hot-path registry write, but distinct operations cannot alias.
pub(crate) fn operation_receipt_ref(
    operation_id: &str,
    request_digest: &str,
    target_ref: &str,
    generation: &str,
) -> Result<brain_protocol::environment::Identifier, EnvironmentError> {
    let mut hasher = Sha256::new();
    for part in [operation_id, request_digest, target_ref, generation] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    format!("receipt:{}", hex::encode(hasher.finalize()))
        .parse()
        .map_err(|_| invalid("operation receipt locator is invalid"))
}

pub(crate) fn target_receipt(target: &TargetSnapshot) -> Result<TargetReceipt, EnvironmentError> {
    Ok(TargetReceipt {
        expires_at_ms: std::num::NonZeroU64::new(target.expires_at_ms)
            .ok_or_else(|| invalid("target expiry is invalid"))?,
        generation: target
            .generation
            .parse()
            .map_err(|_| invalid("generation is invalid"))?,
        target_ref: target
            .target_ref
            .parse()
            .map_err(|_| invalid("target_ref is invalid"))?,
    })
}

pub(crate) fn validate_operation_ref(
    meta: Option<&OperationMeta>,
    operation: &OperationRef,
) -> Result<(), EnvironmentError> {
    match meta {
        Some(meta) if canonical_equal(&meta.operation, operation)? => Ok(()),
        Some(_) => Err(environment_error(
            EnvironmentErrorCode::OperationConflict,
            false,
            "operation locator does not match the reserved receipt",
        )),
        None => Err(operation_error(OperationError::Unknown)),
    }
}

pub(crate) fn validate_wait(wait_ms: u64) -> Result<(), EnvironmentError> {
    if wait_ms > MAX_WAIT_MS {
        Err(invalid(format!("wait exceeds the {MAX_WAIT_MS} ms bound")))
    } else {
        Ok(())
    }
}

pub(crate) fn validate_connector(
    connector: ConnectorClass,
    network: &NetworkCeiling,
    has_proxy: bool,
) -> Result<(), EnvironmentError> {
    let exact = matches!(
        (connector, network, has_proxy),
        (ConnectorClass::None, NetworkCeiling::None, false)
            | (ConnectorClass::Public, NetworkCeiling::Public, false)
            | (
                ConnectorClass::Allowlist,
                NetworkCeiling::Allowlist(_),
                true
            )
    );
    if exact {
        Ok(())
    } else {
        Err(invalid(
            "connector class does not exactly match the root network seal",
        ))
    }
}

pub(crate) fn validate_resource_subset(
    request: &ResourceCeiling,
    physical: &ResourceCeiling,
) -> Result<(), EnvironmentError> {
    ResourceSupport {
        max_timeout_ms: physical.timeout_ms.get().min(MAX_OPERATION_TIMEOUT_MS),
        max_output_bytes: physical
            .max_output_bytes
            .get()
            .min(MAX_OPERATION_OUTPUT_BYTES),
    }
    .validate(ResourceRequest {
        timeout_ms: request.timeout_ms.get(),
        max_output_bytes: request.max_output_bytes.get(),
    })
    .map_err(|error| invalid(error.to_string()))?;
    let within = request.timeout_ms <= physical.timeout_ms
        && request.max_output_bytes <= physical.max_output_bytes;
    if within {
        Ok(())
    } else {
        Err(invalid(
            "operation resources widen the immutable root target seal",
        ))
    }
}

pub(crate) fn canonical_equal<T: serde::Serialize>(left: &T, right: &T) -> Result<bool, EnvironmentError> {
    let left =
        serde_jcs::to_vec(left).map_err(|_| invalid("sealed value is not canonicalizable"))?;
    let right =
        serde_jcs::to_vec(right).map_err(|_| invalid("sealed value is not canonicalizable"))?;
    Ok(left == right)
}
