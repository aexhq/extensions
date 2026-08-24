//! The registry and launcher ports the materializer composes.

use super::*;

/// The reservation lifecycle the [`TargetMaterializer`] composes: acquire a durable reservation,
/// install the launched target, and expire a known-no-effect lease. Every implementation must
/// also uphold the plane capacity invariant: `acquire` charges `spec.materialized_mib` against
/// the plane quota in the same durable transaction as the reservation, and exactly one of
/// `expire_lease` or `mark_closed` refunds it.
#[async_trait]
pub trait TargetReservations: Send + Sync {
    async fn acquire(
        &self,
        request: &AcquireTarget,
    ) -> Result<AcquireOutcome, MaterializationError>;

    async fn install(
        &self,
        lease: &MaterializationLease,
        target: &PhysicalTarget,
        now_ms: u64,
    ) -> Result<InstallOutcome, MaterializationError>;

    /// Removes a known-no-effect failed launch attempt and refunds its charged capacity.
    /// Outcome-unknown launches retain the exact provider request and idempotency token so a
    /// later attempt can recover the same target.
    async fn expire_lease(
        &self,
        lease: &MaterializationLease,
        now_ms: u64,
    ) -> Result<(), MaterializationError>;
}

/// Lifecycle and administrative reads/transitions over installed durable records. `mark_closed`
/// refunds the record's charged plane capacity exactly once.
#[async_trait]
pub trait TargetDirectory: Send + Sync {
    async fn get(
        &self,
        key: &TargetKey,
    ) -> Result<Option<DurableTargetRecord>, MaterializationError>;

    /// Projects an installed record to its closed terminal state.
    async fn mark_closed(
        &self,
        target: &InstalledTarget,
        disposition: Disposition,
        reason: &str,
        now_ms: u64,
    ) -> Result<(), MaterializationError>;

    async fn list_root(
        &self,
        root_id: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<TargetPage, MaterializationError>;
}

/// Convenience bound for stores that provide the complete registry contract.
pub trait DurableTargetRegistry: TargetReservations + TargetDirectory {}
impl<T: TargetReservations + TargetDirectory> DurableTargetRegistry for T {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalTarget {
    pub target_ref: String,
    pub generation: String,
    pub control_token: ControlToken,
}

impl PhysicalTarget {
    pub fn new(
        target_ref: impl Into<String>,
        generation: impl Into<String>,
        control_token: ControlToken,
    ) -> Result<Self, MaterializationError> {
        let target_ref = target_ref.into();
        let generation = generation.into();
        validate_identifier(&target_ref, "target_ref")?;
        validate_identifier(&generation, "generation")?;
        Ok(Self {
            target_ref,
            generation,
            control_token,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LaunchError {
    /// The provider rejected admission because this plane has no allocatable capacity/quota.
    /// No target was created, connector fallback is forbidden, and callers may surface the
    /// bounded retry hint without hiding the saturation signal.
    #[error("provider capacity exhausted ({scope}); retry after {retry_after_ms} ms: {message}")]
    Capacity {
        scope: String,
        retry_after_ms: u64,
        message: String,
    },
    /// The platform attested that no physical target was launched. The lease may be shortened safely.
    #[error("launch rejected before a target was created: {0}")]
    KnownNoTarget(String),
    /// A dependency failed before provider dispatch, so no target exists and capacity may be
    /// refunded, but the same exact materialization is safe to retry later.
    #[error("launch dependency unavailable before a target was created: {0}")]
    RetryableKnownNoTarget(String),
    /// The control call may have launched an idle target. The durable reservation remains charged;
    /// a later short attempt replays the exact provider request and idempotency token.
    #[error("launch outcome is unknown: {0}")]
    OutcomeUnknown(String),
}

#[async_trait]
pub trait PhysicalTargetLauncher: Send + Sync {
    async fn launch(&self, lease: &MaterializationLease) -> Result<PhysicalTarget, LaunchError>;

    /// A worker that loses the install CAS must destroy a different target returned in violation
    /// of the provider idempotency contract. An exact replay normally returns the installed target.
    async fn terminate_stale(&self, target: &PhysicalTarget) -> Result<(), String>;
}
