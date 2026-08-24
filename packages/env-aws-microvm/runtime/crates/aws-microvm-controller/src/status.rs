//! Target key projection and public sandbox status projection.

use crate::*;

pub(crate) fn target_key(target: &SandboxTarget) -> EnvironmentResult<TargetKey> {
    match target.kind {
        TargetKind::Environment if target.sandbox_id.is_none() => {
            TargetKey::for_environment(target.root_id.as_str(), target.binding_ref.as_str())
                .map_err(materialization_error)
        }
        TargetKind::Additional => TargetKey::additional(
            target.root_id.as_str(),
            target
                .sandbox_id
                .as_ref()
                .ok_or_else(|| invalid("additional target requires sandbox_id"))?
                .as_str(),
        )
        .map_err(materialization_error),
        TargetKind::Environment => Err(invalid("environment target cannot carry sandbox_id")),
    }
}

pub(crate) fn environment_target(
    envelope: &brain_protocol::environment::OperationEnvelope,
    binding: &SealedBinding,
) -> EnvironmentResult<SandboxTarget> {
    logical_environment_target(
        envelope.root_id.clone(),
        envelope.session_id.clone(),
        binding.environment_name.as_str(),
    )
}

pub(crate) fn logical_environment_target(
    root_id: brain_protocol::environment::Identifier,
    session_id: brain_protocol::environment::Identifier,
    environment_name: &str,
) -> EnvironmentResult<SandboxTarget> {
    Ok(SandboxTarget {
        binding_ref: brain_protocol::contract::environment_binding_ref(
            root_id.as_str(),
            environment_name,
        ),
        kind: TargetKind::Environment,
        root_id,
        sandbox_id: None,
        session_id,
    })
}

pub(crate) fn status_from_record(
    target: SandboxTarget,
    record: Option<environment_core::materialization::DurableTargetRecord>,
) -> SandboxStatus {
    let Some(record) = record else {
        return SandboxStatus {
            changed_at_ms: None,
            expires_at_ms: None,
            generation: None,
            reason: None,
            state: SandboxState::NeverMaterialized,
            target,
            target_ref: None,
        };
    };
    match record.state {
        DurableTargetState::Materializing {
            target_expires_at_ms,
            ..
        } => SandboxStatus {
            changed_at_ms: Some(record.updated_at_ms),
            expires_at_ms: NonZeroU64::new(target_expires_at_ms),
            generation: Some(record.generation.parse().expect("generation")),
            reason: None,
            state: SandboxState::Creating,
            target,
            target_ref: None,
        },
        DurableTargetState::Installed {
            target_ref,
            expires_at_ms,
            ..
        } => SandboxStatus {
            changed_at_ms: Some(record.updated_at_ms),
            expires_at_ms: NonZeroU64::new(expires_at_ms),
            generation: Some(record.generation.parse().expect("generation")),
            reason: None,
            state: SandboxState::Running,
            target,
            target_ref: Some(target_ref.parse().expect("target ref")),
        },
        DurableTargetState::Closed {
            disposition,
            reason,
            ..
        } => SandboxStatus {
            changed_at_ms: Some(record.updated_at_ms),
            expires_at_ms: None,
            generation: Some(record.generation.parse().expect("generation")),
            reason: Some(reason.parse().expect("reason")),
            state: match disposition {
                Disposition::Lost => SandboxState::Gone,
                Disposition::Terminated => SandboxState::Terminated,
            },
            target,
            target_ref: None,
        },
    }
}

pub(crate) fn sandbox_state_from_provider(
    state: &aws_sdk_lambdamicrovms::types::MicrovmState,
) -> EnvironmentResult<SandboxState> {
    use aws_sdk_lambdamicrovms::types::MicrovmState;

    match state {
        MicrovmState::Running => Ok(SandboxState::Running),
        MicrovmState::Pending => Ok(SandboxState::Creating),
        MicrovmState::Suspended => Ok(SandboxState::Suspended),
        MicrovmState::Suspending => Err(temporary("sandbox suspension is still in progress")),
        MicrovmState::Terminated => Err(error(
            EnvironmentErrorCode::SandboxGone,
            false,
            "provider reports physical generation gone",
        )),
        MicrovmState::Terminating => Err(temporary("sandbox termination is still in progress")),
        // The provider enum is non-exhaustive. A future state must never be reported as running
        // until Environments has explicit routing semantics for it.
        _ => Err(temporary("sandbox provider returned an unsupported state")),
    }
}

pub(crate) fn running_status(target: SandboxTarget, installed: &InstalledTarget) -> SandboxStatus {
    SandboxStatus {
        changed_at_ms: Some(installed.installed_at_ms),
        expires_at_ms: NonZeroU64::new(installed.expires_at_ms),
        generation: Some(installed.generation.parse().expect("generation")),
        reason: None,
        state: SandboxState::Running,
        target,
        target_ref: Some(installed.target_ref.parse().expect("target ref")),
    }
}

pub(crate) fn gone_status(
    target: SandboxTarget,
    installed: &InstalledTarget,
    reason: &str,
) -> SandboxStatus {
    SandboxStatus {
        changed_at_ms: Some(now_ms()),
        expires_at_ms: None,
        generation: Some(installed.generation.parse().expect("generation")),
        reason: Some(reason.parse().expect("reason")),
        state: SandboxState::Gone,
        target,
        target_ref: None,
    }
}

pub(crate) fn terminated_status(
    target: SandboxTarget,
    installed: &InstalledTarget,
    reason: &str,
) -> SandboxStatus {
    SandboxStatus {
        changed_at_ms: Some(now_ms()),
        expires_at_ms: None,
        generation: Some(installed.generation.parse().expect("generation")),
        reason: Some(reason.parse().expect("reason")),
        state: SandboxState::Terminated,
        target,
        target_ref: None,
    }
}

pub(crate) fn target_identity(key: &TargetKey) -> EnvironmentResult<String> {
    key.target_identity()
        .map(str::to_owned)
        .map_err(|_| invalid("target key has an unrecognized shape"))
}

pub(crate) fn random_identifier(prefix: &str) -> String {
    format!("{prefix}-{}", hex::encode(rand::random::<[u8; 16]>()))
}

// Fail closed: every expiry predicate in this crate compares against this value, so a pre-epoch
// clock must abort rather than make every expired authority look valid.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the UNIX epoch")
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_receipts_route_to_the_logical_environment_target() {
        let root_id = "ses_root".parse().unwrap();
        let session_id = "ses_child".parse().unwrap();
        let target = logical_environment_target(root_id, session_id, "workspace").unwrap();
        assert_eq!(
            target.binding_ref,
            brain_protocol::contract::environment_binding_ref("ses_root", "workspace")
        );
        assert_ne!(target.binding_ref.as_str(), "binding:tool");
        assert_eq!(target.kind, TargetKind::Environment);
        assert!(target.sandbox_id.is_none());
    }
}
