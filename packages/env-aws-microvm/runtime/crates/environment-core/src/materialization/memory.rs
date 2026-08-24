//! In-memory registry double with plane-quota semantics (test support only).

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::*;

fn poisoned() -> MaterializationError {
    MaterializationError::Storage("target registry lock is poisoned".into())
}

/// In-memory conformance implementation. Production uses the DynamoDB implementation, while this
/// one makes every transition and crash point deterministic in tests.
#[derive(Debug)]
pub struct MemoryTargetRegistry {
    records: Mutex<BTreeMap<TargetKey, DurableTargetRecord>>,
    capacity: Mutex<PlaneAllocation>,
}

#[derive(Debug)]
struct PlaneAllocation {
    max_mib: u64,
    reserved_mib: u64,
}

impl Default for MemoryTargetRegistry {
    fn default() -> Self {
        Self::with_capacity(u64::MAX)
    }
}

impl MemoryTargetRegistry {
    #[must_use]
    pub fn with_capacity(max_mib: u64) -> Self {
        assert!(max_mib > 0);
        Self {
            records: Mutex::new(BTreeMap::new()),
            capacity: Mutex::new(PlaneAllocation {
                max_mib,
                reserved_mib: 0,
            }),
        }
    }

    fn reserve_capacity(&self, mib: u64) -> Result<(), MaterializationError> {
        let mut capacity = self.capacity.lock().map_err(|_| poisoned())?;
        if capacity.reserved_mib > capacity.max_mib.saturating_sub(mib) {
            return Err(MaterializationError::Capacity {
                scope: "plane_materialized_memory_mib".into(),
                retry_after_ms: 1_000,
                message: format!("{mib} MiB target exceeds remaining plane allocation"),
            });
        }
        capacity.reserved_mib += mib;
        Ok(())
    }

    fn refund_capacity(&self, mib: u64) -> Result<(), MaterializationError> {
        let mut capacity = self.capacity.lock().map_err(|_| poisoned())?;
        capacity.reserved_mib = capacity
            .reserved_mib
            .checked_sub(mib)
            .ok_or_else(|| MaterializationError::Corrupt("capacity counter underflow".into()))?;
        Ok(())
    }

    #[must_use]
    pub fn reserved_mib(&self) -> u64 {
        self.capacity.lock().expect("capacity lock").reserved_mib
    }
}

#[async_trait]
impl TargetReservations for MemoryTargetRegistry {
    async fn acquire(
        &self,
        request: &AcquireTarget,
    ) -> Result<AcquireOutcome, MaterializationError> {
        let lease = request.lease()?;
        let mut records = self.records.lock().map_err(|_| poisoned())?;
        let Some(record) = records.get_mut(&request.key) else {
            self.reserve_capacity(lease.spec.materialized_mib)?;
            records.insert(
                request.key.clone(),
                record_from_lease(&lease, request.now_ms),
            );
            return Ok(AcquireOutcome::Acquired(lease));
        };
        record.validate()?;
        if record.spec_digest != lease.spec_digest {
            return Err(MaterializationError::SpecConflict);
        }
        if request.generation_is_fenced
            && record.generation != lease.generation
            && !(request.replace_after_loss
                && matches!(
                    &record.state,
                    DurableTargetState::Closed {
                        disposition: Disposition::Lost,
                        ..
                    }
                ))
        {
            return Err(MaterializationError::SpecConflict);
        }
        match record.state.clone() {
            DurableTargetState::Installed { .. } => Ok(AcquireOutcome::Installed(
                record.installed().expect("installed state projects"),
            )),
            DurableTargetState::Materializing {
                lease_expires_at_ms,
                ..
            } if lease_expires_at_ms <= request.now_ms => {
                // The provider's complete possible target lifetime plus skew has elapsed. The
                // old charged slot is now safe to reuse for a newly sealed exact request.
                *record = record_from_lease(&lease, request.now_ms);
                Ok(AcquireOutcome::Acquired(lease))
            }
            DurableTargetState::Materializing {
                target_expires_at_ms,
                lease_expires_at_ms,
                ..
            } if target_expires_at_ms <= request.now_ms => Ok(AcquireOutcome::Pending {
                generation: record.generation.clone(),
                retry_after_ms: materialization_poll_after(lease_expires_at_ms, request.now_ms),
            }),
            DurableTargetState::Materializing {
                attempt_expires_at_ms,
                ..
            } if attempt_expires_at_ms > request.now_ms => Ok(AcquireOutcome::Pending {
                generation: record.generation.clone(),
                retry_after_ms: materialization_poll_after(attempt_expires_at_ms, request.now_ms),
            }),
            DurableTargetState::Materializing { .. } => Ok(AcquireOutcome::Acquired(
                take_materialization_attempt(record, request)?,
            )),
            DurableTargetState::Closed {
                disposition: Disposition::Lost,
                ..
            } if request.replace_after_loss => {
                self.reserve_capacity(lease.spec.materialized_mib)?;
                *record = record_from_lease(&lease, request.now_ms);
                Ok(AcquireOutcome::Acquired(lease))
            }
            DurableTargetState::Closed {
                disposition: Disposition::Lost,
                ..
            } => Ok(AcquireOutcome::Gone),
            DurableTargetState::Closed {
                disposition: Disposition::Terminated,
                ..
            } => Ok(AcquireOutcome::Terminated),
        }
    }

    async fn install(
        &self,
        lease: &MaterializationLease,
        target: &PhysicalTarget,
        now_ms: u64,
    ) -> Result<InstallOutcome, MaterializationError> {
        validate_identifier(&target.target_ref, "target_ref")?;
        validate_identifier(&target.generation, "generation")?;
        let mut records = self.records.lock().map_err(|_| poisoned())?;
        let Some(record) = records.get_mut(&lease.key) else {
            return Ok(InstallOutcome::ReservationLost);
        };
        if record.spec_digest != lease.spec_digest || record.generation != lease.generation {
            return Ok(InstallOutcome::ReservationLost);
        }
        match &record.state {
            DurableTargetState::Materializing { reservation_id, .. }
                if reservation_id == &lease.reservation_id =>
            {
                record.state = DurableTargetState::Installed {
                    target_ref: target.target_ref.clone(),
                    control_token: target.control_token.clone(),
                    installed_at_ms: now_ms,
                    expires_at_ms: lease.target_expires_at_ms,
                };
                record.generation = target.generation.clone();
                record.updated_at_ms = now_ms;
                Ok(InstallOutcome::Installed(
                    record.installed().expect("installed state projects"),
                ))
            }
            DurableTargetState::Installed {
                target_ref: existing,
                control_token,
                ..
            } if existing == &target.target_ref
                && control_token == &target.control_token
                && record.generation == target.generation =>
            {
                Ok(InstallOutcome::Installed(
                    record.installed().expect("installed state projects"),
                ))
            }
            _ => Ok(InstallOutcome::ReservationLost),
        }
    }

    async fn expire_lease(
        &self,
        lease: &MaterializationLease,
        now_ms: u64,
    ) -> Result<(), MaterializationError> {
        let mut records = self.records.lock().map_err(|_| poisoned())?;
        let release = if let Some(record) = records.get(&lease.key)
            && record.generation == lease.generation
            && matches!(
                &record.state,
                DurableTargetState::Materializing {
                    reservation_id,
                    attempt_id,
                    ..
                } if reservation_id == &lease.reservation_id && attempt_id == &lease.attempt_id
            ) {
            true
        } else {
            false
        };
        if release {
            records.remove(&lease.key);
            self.refund_capacity(lease.spec.materialized_mib)?;
        }
        let _ = now_ms;
        Ok(())
    }
}

#[async_trait]
impl TargetDirectory for MemoryTargetRegistry {
    async fn get(
        &self,
        key: &TargetKey,
    ) -> Result<Option<DurableTargetRecord>, MaterializationError> {
        key.validate()?;
        Ok(self
            .records
            .lock()
            .map_err(|_| poisoned())?
            .get(key)
            .cloned())
    }

    async fn mark_closed(
        &self,
        target: &InstalledTarget,
        disposition: Disposition,
        reason: &str,
        now_ms: u64,
    ) -> Result<(), MaterializationError> {
        validate_reason(reason)?;
        let mut records = self.records.lock().map_err(|_| poisoned())?;
        let was_installed = records.get(&target.key).is_some_and(|record| {
            matches!(&record.state, DurableTargetState::Installed { target_ref, .. } if target_ref == &target.target_ref)
                && record.generation == target.generation
        });
        transition_installed(&mut records, target, |record| {
            record.state = DurableTargetState::Closed {
                disposition,
                reason: reason.into(),
                at_ms: now_ms,
            };
            record.updated_at_ms = now_ms;
        })?;
        if was_installed {
            self.refund_capacity(target.spec.materialized_mib)?;
        }
        Ok(())
    }

    async fn list_root(
        &self,
        root_id: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<TargetPage, MaterializationError> {
        validate_identifier(root_id, "root_id")?;
        let limit = limit.clamp(1, MAX_TARGET_PAGE);
        let records = self.records.lock().map_err(|_| poisoned())?;
        let items: Vec<_> = records
            .values()
            .filter(|record| {
                record.key.root_id == root_id
                    && cursor.is_none_or(|cursor| record.key.target_key.as_str() > cursor)
            })
            .take(limit + 1)
            .cloned()
            .collect();
        Ok(crate::page::page(items, limit))
    }
}

pub(crate) fn record_from_lease(lease: &MaterializationLease, now_ms: u64) -> DurableTargetRecord {
    DurableTargetRecord {
        key: lease.key.clone(),
        spec: lease.spec.clone(),
        spec_digest: lease.spec_digest.clone(),
        generation: lease.generation.clone(),
        state: DurableTargetState::Materializing {
            reservation_id: lease.reservation_id.clone(),
            launch_request: lease.launch_request.clone(),
            attempt_id: lease.attempt_id.clone(),
            attempt_expires_at_ms: lease.attempt_expires_at_ms,
            target_expires_at_ms: lease.target_expires_at_ms,
            lease_expires_at_ms: lease.lease_expires_at_ms,
        },
        updated_at_ms: now_ms,
    }
}
pub(crate) fn take_materialization_attempt(
    record: &mut DurableTargetRecord,
    request: &AcquireTarget,
) -> Result<MaterializationLease, MaterializationError> {
    let DurableTargetState::Materializing {
        attempt_id,
        attempt_expires_at_ms,
        lease_expires_at_ms,
        ..
    } = &mut record.state
    else {
        return Err(MaterializationError::Corrupt(
            "expected materializing target".into(),
        ));
    };
    *attempt_id = request.attempt_id.clone();
    *attempt_expires_at_ms = request
        .now_ms
        .checked_add(request.attempt_duration_ms)
        .ok_or(MaterializationError::InvalidLease)?
        .min(*lease_expires_at_ms);
    record.updated_at_ms = request.now_ms;
    record.validate()?;
    lease_from_materializing_record(record)
}
pub(crate) fn transition_installed(
    records: &mut BTreeMap<TargetKey, DurableTargetRecord>,
    target: &InstalledTarget,
    transition: impl FnOnce(&mut DurableTargetRecord),
) -> Result<(), MaterializationError> {
    let record = records
        .get_mut(&target.key)
        .ok_or(MaterializationError::ReservationLost { cleanup: None })?;
    match &record.state {
        DurableTargetState::Installed { target_ref, .. }
            if target_ref == &target.target_ref && record.generation == target.generation =>
        {
            transition(record);
            Ok(())
        }
        DurableTargetState::Closed { .. } => Ok(()),
        _ => Err(MaterializationError::ReservationLost { cleanup: None }),
    }
}
