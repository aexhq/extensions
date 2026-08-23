//! The reservation → launch → install orchestrator.

use super::*;

/// Coordinates one target launch. It only returns after physical routing is durably installed.
pub struct TargetMaterializer<R, L> {
    registry: R,
    launcher: L,
}

impl<R, L> TargetMaterializer<R, L> {
    pub const fn new(registry: R, launcher: L) -> Self {
        Self { registry, launcher }
    }

    #[must_use]
    pub const fn registry(&self) -> &R {
        &self.registry
    }
}

impl<R, L> TargetMaterializer<R, L>
where
    R: TargetReservations,
    L: PhysicalTargetLauncher,
{
    /// No caller may dispatch a guest effect until this returns `InstalledTarget`.
    pub async fn ensure(
        &self,
        request: &AcquireTarget,
    ) -> Result<InstalledTarget, MaterializationError> {
        let lease = match self.registry.acquire(request).await? {
            AcquireOutcome::Installed(target) => return Ok(target),
            AcquireOutcome::Acquired(lease) => lease,
            AcquireOutcome::Pending { retry_after_ms, .. } => {
                return Err(MaterializationError::Pending { retry_after_ms });
            }
            AcquireOutcome::Gone => return Err(MaterializationError::Gone),
            AcquireOutcome::Terminated => return Err(MaterializationError::Terminated),
        };

        let physical = match self.launcher.launch(&lease).await {
            Ok(target) => target,
            Err(LaunchError::Capacity {
                scope,
                retry_after_ms,
                message,
            }) => {
                if !lease.recovery_attempt {
                    self.registry.expire_lease(&lease, request.now_ms).await?;
                }
                return Err(MaterializationError::Capacity {
                    scope,
                    retry_after_ms,
                    message,
                });
            }
            Err(LaunchError::KnownNoTarget(message)) => {
                if lease.recovery_attempt {
                    return Err(MaterializationError::LaunchOutcomeUnknown(format!(
                        "exact launch recovery returned no target; reservation remains fenced: {message}"
                    )));
                }
                self.registry.expire_lease(&lease, request.now_ms).await?;
                return Err(MaterializationError::LaunchRejected(message));
            }
            Err(LaunchError::RetryableKnownNoTarget(message)) => {
                if !lease.recovery_attempt {
                    self.registry.expire_lease(&lease, request.now_ms).await?;
                }
                return Err(MaterializationError::LaunchRetryable(message));
            }
            Err(LaunchError::OutcomeUnknown(message)) => {
                return Err(MaterializationError::LaunchOutcomeUnknown(message));
            }
        };

        match self
            .registry
            .install(&lease, &physical, request.now_ms)
            .await?
        {
            InstallOutcome::Installed(target) => Ok(target),
            InstallOutcome::ReservationLost => {
                let cleanup = self.launcher.terminate_stale(&physical).await.err();
                Err(MaterializationError::ReservationLost { cleanup })
            }
        }
    }
}
