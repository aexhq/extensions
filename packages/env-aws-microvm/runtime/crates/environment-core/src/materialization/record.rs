//! Durable target records, leases, and state transitions.

use super::*;

// Deliberately not serde-serializable: `recovery_attempt` has no safe deserialization default
// (a deserialized `false` would let recovery treat a possibly-dispatched launch as fresh), and
// leases are always rebuilt field-by-field from durable records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializationLease {
    pub key: TargetKey,
    pub spec: TargetSpec,
    pub spec_digest: String,
    pub reservation_id: String,
    pub generation: String,
    pub launch_request: DurableLaunchRequest,
    /// Short ownership lease for one exact provider attempt. It is independent of the long
    /// uncertainty fence below: takeover retries the same client token and provider request.
    pub attempt_id: String,
    pub attempt_expires_at_ms: u64,
    /// Conservative provider hard deadline, measured from reservation before Run is dispatched.
    pub target_expires_at_ms: u64,
    pub lease_expires_at_ms: u64,
    /// Ephemeral attempt provenance. `false` means this worker installed the durable reservation
    /// before its first provider dispatch. `true` means a prior worker may already have
    /// dispatched the exact sealed provider request, so an error from the idempotent replay can
    /// never be used as proof that no physical target exists.
    pub recovery_attempt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledTarget {
    pub key: TargetKey,
    pub spec: TargetSpec,
    pub spec_digest: String,
    pub target_ref: String,
    pub generation: String,
    pub control_token: ControlToken,
    pub installed_at_ms: u64,
    pub expires_at_ms: u64,
}

impl InstalledTarget {
    pub fn validate(&self) -> Result<(), MaterializationError> {
        self.key.validate()?;
        validate_digest(&self.spec_digest, "spec_digest")?;
        if self.spec.digest() != self.spec_digest {
            return Err(MaterializationError::SpecConflict);
        }
        validate_identifier(&self.target_ref, "target_ref")?;
        validate_identifier(&self.generation, "generation")?;
        if self.expires_at_ms == 0 || self.expires_at_ms < self.installed_at_ms {
            return Err(MaterializationError::InvalidLease);
        }
        Ok(())
    }
}

/// Why a closed target reached its terminal state. The stored disposition decides whether the
/// named environment target may be replaced after confirmed loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// The provider lost the physical target (crash, hard deadline, external loss).
    Lost,
    /// An explicit lifecycle operation deliberately terminated the target.
    Terminated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableTargetState {
    Materializing {
        reservation_id: String,
        launch_request: DurableLaunchRequest,
        attempt_id: String,
        attempt_expires_at_ms: u64,
        target_expires_at_ms: u64,
        lease_expires_at_ms: u64,
    },
    Installed {
        target_ref: String,
        control_token: ControlToken,
        installed_at_ms: u64,
        expires_at_ms: u64,
    },
    Closed {
        disposition: Disposition,
        reason: String,
        at_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableTargetRecord {
    pub key: TargetKey,
    pub spec: TargetSpec,
    pub spec_digest: String,
    pub generation: String,
    pub state: DurableTargetState,
    pub updated_at_ms: u64,
}

impl DurableTargetRecord {
    pub fn validate(&self) -> Result<(), MaterializationError> {
        self.key.validate()?;
        validate_digest(&self.spec_digest, "spec_digest")?;
        if self.spec.digest() != self.spec_digest {
            return Err(MaterializationError::SpecConflict);
        }
        validate_identifier(&self.generation, "generation")?;
        match &self.state {
            DurableTargetState::Materializing {
                reservation_id,
                launch_request,
                attempt_id,
                attempt_expires_at_ms,
                target_expires_at_ms,
                lease_expires_at_ms,
            } => {
                validate_identifier(reservation_id, "reservation_id")?;
                launch_request.validate()?;
                validate_identifier(attempt_id, "attempt_id")?;
                if *target_expires_at_ms == 0
                    || *target_expires_at_ms >= *lease_expires_at_ms
                    || *target_expires_at_ms < self.updated_at_ms
                    || *attempt_expires_at_ms <= self.updated_at_ms
                    || *attempt_expires_at_ms > *lease_expires_at_ms
                {
                    return Err(MaterializationError::InvalidLease);
                }
                Ok(())
            }
            DurableTargetState::Installed {
                target_ref,
                installed_at_ms,
                expires_at_ms,
                ..
            } => {
                validate_identifier(target_ref, "target_ref")?;
                if *expires_at_ms == 0 || *expires_at_ms < *installed_at_ms {
                    return Err(MaterializationError::InvalidLease);
                }
                Ok(())
            }
            DurableTargetState::Closed { reason, .. } => validate_reason(reason),
        }
    }

    #[must_use]
    pub fn installed(&self) -> Option<InstalledTarget> {
        let DurableTargetState::Installed {
            target_ref,
            control_token,
            installed_at_ms,
            expires_at_ms,
        } = &self.state
        else {
            return None;
        };
        Some(InstalledTarget {
            key: self.key.clone(),
            spec: self.spec.clone(),
            spec_digest: self.spec_digest.clone(),
            target_ref: target_ref.clone(),
            generation: self.generation.clone(),
            control_token: control_token.clone(),
            installed_at_ms: *installed_at_ms,
            expires_at_ms: *expires_at_ms,
        })
    }

    /// Reconstructs the exact durable provider attempt for reconciliation. A lease obtained from
    /// a record is always a recovery attempt: even when this process has not called the provider,
    /// an earlier owner may have dispatched the same idempotency token before it crashed.
    pub fn recovery_lease(&self) -> Result<MaterializationLease, MaterializationError> {
        lease_from_materializing_record(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquireTarget {
    pub key: TargetKey,
    pub spec: TargetSpec,
    pub reservation_id: String,
    pub generation: String,
    pub launch_request: DurableLaunchRequest,
    pub attempt_id: String,
    pub attempt_duration_ms: u64,
    /// Brain-minted create operations fence on this generation. Lazy managed execution lets Environment
    /// mint it and therefore accepts the already-installed generation on a retry.
    pub generation_is_fenced: bool,
    pub now_ms: u64,
    pub lease_duration_ms: u64,
    /// Maximum physical target lifetime. It must fit strictly inside the uncertainty lease.
    pub target_lifetime_ms: u64,
    /// Only a named environment target may get a fresh generation after confirmed loss.
    pub replace_after_loss: bool,
}

impl AcquireTarget {
    pub fn validate(&self) -> Result<(), MaterializationError> {
        self.key.validate()?;
        validate_identifier(&self.reservation_id, "reservation_id")?;
        validate_identifier(&self.generation, "generation")?;
        self.launch_request.validate()?;
        validate_identifier(&self.attempt_id, "attempt_id")?;
        if self.target_lifetime_ms == 0 || self.lease_duration_ms <= self.target_lifetime_ms {
            return Err(MaterializationError::InvalidLease);
        }
        if self.attempt_duration_ms == 0 || self.attempt_duration_ms >= self.lease_duration_ms {
            return Err(MaterializationError::InvalidLease);
        }
        if self.replace_after_loss && !self.key.is_environment() {
            return Err(MaterializationError::InvalidReplacement);
        }
        Ok(())
    }

    pub fn lease(&self) -> Result<MaterializationLease, MaterializationError> {
        self.validate()?;
        let lease_expires_at_ms = self
            .now_ms
            .checked_add(self.lease_duration_ms)
            .ok_or(MaterializationError::InvalidLease)?;
        let target_expires_at_ms = self
            .now_ms
            .checked_add(self.target_lifetime_ms)
            .ok_or(MaterializationError::InvalidLease)?;
        let attempt_expires_at_ms = self
            .now_ms
            .checked_add(self.attempt_duration_ms)
            .ok_or(MaterializationError::InvalidLease)?;
        Ok(MaterializationLease {
            key: self.key.clone(),
            spec: self.spec.clone(),
            spec_digest: self.spec.digest(),
            reservation_id: self.reservation_id.clone(),
            generation: self.generation.clone(),
            launch_request: self.launch_request.clone(),
            attempt_id: self.attempt_id.clone(),
            attempt_expires_at_ms,
            target_expires_at_ms,
            lease_expires_at_ms,
            recovery_attempt: false,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireOutcome {
    Acquired(MaterializationLease),
    Pending {
        generation: String,
        retry_after_ms: u64,
    },
    Installed(InstalledTarget),
    Gone,
    Terminated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// This transition sits on first materialization only. Keeping the installed proof inline avoids a
// second allocation and makes the returned CAS evidence one owned value.
#[allow(clippy::large_enum_variant)]
pub enum InstallOutcome {
    Installed(InstalledTarget),
    ReservationLost,
}

pub type TargetPage = crate::page::Page<DurableTargetRecord>;

impl crate::page::PageIdentity for DurableTargetRecord {
    fn page_identity(&self) -> &str {
        &self.key.target_key
    }
}

pub(crate) fn lease_from_materializing_record(
    record: &DurableTargetRecord,
) -> Result<MaterializationLease, MaterializationError> {
    let DurableTargetState::Materializing {
        reservation_id,
        launch_request,
        attempt_id,
        attempt_expires_at_ms,
        target_expires_at_ms,
        lease_expires_at_ms,
    } = &record.state
    else {
        return Err(MaterializationError::Corrupt(
            "expected materializing target".into(),
        ));
    };
    Ok(MaterializationLease {
        key: record.key.clone(),
        spec: record.spec.clone(),
        spec_digest: record.spec_digest.clone(),
        reservation_id: reservation_id.clone(),
        generation: record.generation.clone(),
        launch_request: launch_request.clone(),
        attempt_id: attempt_id.clone(),
        attempt_expires_at_ms: *attempt_expires_at_ms,
        target_expires_at_ms: *target_expires_at_ms,
        lease_expires_at_ms: *lease_expires_at_ms,
        recovery_attempt: true,
    })
}

#[must_use]
pub fn materialization_poll_after(lease_expires_at_ms: u64, now_ms: u64) -> u64 {
    lease_expires_at_ms
        .saturating_sub(now_ms)
        .clamp(1, MAX_MATERIALIZATION_POLL_MS)
}
