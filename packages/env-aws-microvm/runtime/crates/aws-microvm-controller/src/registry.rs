//! Plane-local DynamoDB registry for durable physical target routing.
//!
//! The table has `root_id` (partition key) and `target_key` (sort key). It has no GSI or stream.
//! Operation rows do not belong here: Brain has already committed intent and the installed guest
//! owns operation deduplication. Target tombstones remain until explicit root deletion so an
//! additional sandbox ID can never silently rematerialize after a TTL delay.

use std::collections::HashMap;

use crate::dynamo::{conditional_failure, n, s};
use async_trait::async_trait;
use aws_sdk_dynamodb::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_dynamodb::primitives::Blob;
use aws_sdk_dynamodb::types::{
    AttributeValue, Delete, Put, ReturnValue, TransactWriteItem, Update,
};
use environment_core::connector::ConnectorClass;
use environment_core::materialization::{
    AcquireOutcome, AcquireTarget, ControlToken, Disposition, DurableLaunchRequest,
    DurableTargetRecord, DurableTargetState, InstallOutcome, InstalledTarget, MAX_REASON_BYTES,
    MAX_TARGET_PAGE, MaterializationError, MaterializationLease, TARGET_KEY_PREFIX,
    TargetDirectory, TargetKey, TargetPage, TargetReservations, TargetSpec,
    materialization_poll_after,
};

const ROOT_ID: &str = "root_id";
const TARGET_KEY: &str = "target_key";
const STATE: &str = "state";
const MATERIALIZING: &str = "materializing";
const INSTALLED: &str = "installed";
const CLOSED: &str = "closed";
const DISPOSITION_LOST: &str = "lost";
const DISPOSITION_TERMINATED: &str = "terminated";
const LAUNCH_REQUEST: &str = "launch_request";
const CONTROL_TOKEN: &str = "control_token";
const ATTEMPT_ID: &str = "attempt_id";
const ATTEMPT_EXPIRES_AT_MS: &str = "attempt_expires_at_ms";
const CAPACITY_ROOT_ID: &str = "plane";
const CAPACITY_TARGET_KEY: &str = "capacity:materialized_mib";

/// Strongly consistent target registry. Clone shares the AWS SDK connection pool.
#[derive(Clone)]
pub struct DynamoTargetRegistry {
    db: aws_sdk_dynamodb::Client,
    table: String,
    max_materialized_mib: u64,
}

impl DynamoTargetRegistry {
    pub fn new(
        db: aws_sdk_dynamodb::Client,
        table: impl Into<String>,
        max_materialized_mib: u64,
    ) -> Self {
        assert!(
            max_materialized_mib > 0,
            "materialization capacity must be positive"
        );
        Self {
            db,
            table: table.into(),
            max_materialized_mib,
        }
    }

    async fn read(
        &self,
        key: &TargetKey,
    ) -> Result<Option<DurableTargetRecord>, MaterializationError> {
        let output = self
            .db
            .get_item()
            .table_name(&self.table)
            .key(ROOT_ID, AttributeValue::S(key.root_id.clone()))
            .key(TARGET_KEY, AttributeValue::S(key.target_key.clone()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| storage_error("get target", &error))?;
        output.item().map(parse_record).transpose()
    }

    async fn reserved_capacity(&self) -> Result<u64, MaterializationError> {
        let output = self
            .db
            .get_item()
            .table_name(&self.table)
            .set_key(Some(capacity_key()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| storage_error("get materialization capacity", &error))?;
        let Some(item) = output.item() else {
            return Ok(0);
        };
        item.get("reserved_mib")
            .and_then(|value| value.as_n().ok())
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| corrupt("capacity row has no valid reserved_mib"))
    }

    async fn replace_expired_materialization(
        &self,
        current: &DurableTargetRecord,
        request: &AcquireTarget,
        lease: &MaterializationLease,
    ) -> Result<bool, MaterializationError> {
        let result = self
            .db
            .update_item()
            .table_name(&self.table)
            .key(ROOT_ID, AttributeValue::S(request.key.root_id.clone()))
            .key(
                TARGET_KEY,
                AttributeValue::S(request.key.target_key.clone()),
            )
            .condition_expression(
                "#state = :materializing AND reservation_id = :old_reservation \
                 AND lease_expires_at_ms <= :now AND spec_digest = :spec_digest",
            )
            .update_expression(
                "SET #state = :materializing, reservation_id = :reservation, \
                 generation = :generation, lease_expires_at_ms = :lease, \
                 expires_at_ms = :expires, launch_request = :launch_request, \
                 attempt_id = :attempt_id, attempt_expires_at_ms = :attempt_expires, \
                 updated_at_ms = :now \
                 REMOVE target_ref, control_token, installed_at_ms, reason, disposition, at_ms, \
                 expires_at_s",
            )
            .expression_attribute_names("#state", STATE)
            .expression_attribute_values(":materializing", s(MATERIALIZING))
            .expression_attribute_values(":old_reservation", s(materializing_reservation(current)?))
            .expression_attribute_values(":now", n(request.now_ms))
            .expression_attribute_values(":spec_digest", s(&lease.spec_digest))
            .expression_attribute_values(":reservation", s(&lease.reservation_id))
            .expression_attribute_values(":generation", s(&lease.generation))
            .expression_attribute_values(":lease", n(lease.lease_expires_at_ms))
            .expression_attribute_values(":expires", n(lease.target_expires_at_ms))
            .expression_attribute_values(
                ":launch_request",
                AttributeValue::B(Blob::new(lease.launch_request.expose().as_bytes())),
            )
            .expression_attribute_values(":attempt_id", s(&lease.attempt_id))
            .expression_attribute_values(":attempt_expires", n(lease.attempt_expires_at_ms))
            .send()
            .await;
        conditional_result("replace expired materialization", result)
    }

    async fn take_expired_attempt(
        &self,
        current: &DurableTargetRecord,
        request: &AcquireTarget,
    ) -> Result<Option<MaterializationLease>, MaterializationError> {
        let DurableTargetState::Materializing {
            reservation_id,
            launch_request,
            attempt_id,
            target_expires_at_ms,
            lease_expires_at_ms,
            ..
        } = &current.state
        else {
            return Err(corrupt("expected materializing target"));
        };
        let attempt_expires_at_ms = request
            .now_ms
            .checked_add(request.attempt_duration_ms)
            .ok_or(MaterializationError::InvalidLease)?
            .min(*lease_expires_at_ms);
        if attempt_expires_at_ms <= request.now_ms {
            return Err(MaterializationError::InvalidLease);
        }
        let result = self
            .db
            .update_item()
            .table_name(&self.table)
            .key(ROOT_ID, AttributeValue::S(request.key.root_id.clone()))
            .key(
                TARGET_KEY,
                AttributeValue::S(request.key.target_key.clone()),
            )
            .condition_expression(
                "#state = :materializing AND reservation_id = :reservation \
                 AND generation = :generation AND attempt_id = :old_attempt \
                 AND attempt_expires_at_ms <= :now AND lease_expires_at_ms > :now \
                 AND expires_at_ms > :now \
                 AND spec_digest = :spec_digest",
            )
            .update_expression(
                "SET attempt_id = :attempt, attempt_expires_at_ms = :attempt_expires, \
                 updated_at_ms = :now",
            )
            .expression_attribute_names("#state", STATE)
            .expression_attribute_values(":materializing", s(MATERIALIZING))
            .expression_attribute_values(":reservation", s(reservation_id))
            .expression_attribute_values(":generation", s(&current.generation))
            .expression_attribute_values(":old_attempt", s(attempt_id))
            .expression_attribute_values(":now", n(request.now_ms))
            .expression_attribute_values(":spec_digest", s(&current.spec_digest))
            .expression_attribute_values(":attempt", s(&request.attempt_id))
            .expression_attribute_values(":attempt_expires", n(attempt_expires_at_ms))
            .send()
            .await;
        if !conditional_result("take materialization attempt", result)? {
            return Ok(None);
        }
        Ok(Some(MaterializationLease {
            key: current.key.clone(),
            spec: current.spec.clone(),
            spec_digest: current.spec_digest.clone(),
            reservation_id: reservation_id.clone(),
            generation: current.generation.clone(),
            launch_request: launch_request.clone(),
            attempt_id: request.attempt_id.clone(),
            attempt_expires_at_ms,
            target_expires_at_ms: *target_expires_at_ms,
            lease_expires_at_ms: *lease_expires_at_ms,
            recovery_attempt: true,
        }))
    }

    async fn replace_gone_environment(
        &self,
        current: &DurableTargetRecord,
        request: &AcquireTarget,
        lease: &MaterializationLease,
    ) -> Result<bool, MaterializationError> {
        let update = Update::builder()
            .table_name(&self.table)
            .key(ROOT_ID, AttributeValue::S(request.key.root_id.clone()))
            .key(
                TARGET_KEY,
                AttributeValue::S(request.key.target_key.clone()),
            )
            .condition_expression(
                "#state = :closed AND disposition = :lost AND generation = :old_generation \
                 AND spec_digest = :spec_digest",
            )
            .update_expression(
                "SET #state = :materializing, reservation_id = :reservation, \
                 generation = :generation, lease_expires_at_ms = :lease, \
                 expires_at_ms = :expires, launch_request = :launch_request, \
                 attempt_id = :attempt_id, attempt_expires_at_ms = :attempt_expires, \
                 updated_at_ms = :now \
                 REMOVE target_ref, control_token, installed_at_ms, reason, disposition, at_ms, \
                 expires_at_s",
            )
            .expression_attribute_names("#state", STATE)
            .expression_attribute_values(":closed", s(CLOSED))
            .expression_attribute_values(":lost", s(DISPOSITION_LOST))
            .expression_attribute_values(":old_generation", s(&current.generation))
            .expression_attribute_values(":spec_digest", s(&lease.spec_digest))
            .expression_attribute_values(":materializing", s(MATERIALIZING))
            .expression_attribute_values(":reservation", s(&lease.reservation_id))
            .expression_attribute_values(":generation", s(&lease.generation))
            .expression_attribute_values(":lease", n(lease.lease_expires_at_ms))
            .expression_attribute_values(":expires", n(lease.target_expires_at_ms))
            .expression_attribute_values(
                ":launch_request",
                AttributeValue::B(Blob::new(lease.launch_request.expose().as_bytes())),
            )
            .expression_attribute_values(":attempt_id", s(&lease.attempt_id))
            .expression_attribute_values(":attempt_expires", n(lease.attempt_expires_at_ms))
            .expression_attribute_values(":now", n(request.now_ms))
            .build()
            .map_err(|error| {
                MaterializationError::Storage(format!("replace target build: {error}"))
            })?;
        let capacity = capacity_add_update(
            &self.table,
            lease.spec.materialized_mib,
            self.max_materialized_mib,
            request.now_ms,
        )?;
        let result = self
            .db
            .transact_write_items()
            .transact_items(TransactWriteItem::builder().update(update).build())
            .transact_items(TransactWriteItem::builder().update(capacity).build())
            .send()
            .await;
        transaction_result("replace gone environment", result)
    }

    /// Removes one already-unaccounted terminal target during explicit root deletion. Capacity
    /// was decremented by the gone/terminated transition; this exact conditional delete must not
    /// touch the plane counter again.
    pub async fn purge_terminal(
        &self,
        key: &TargetKey,
        generation: &str,
    ) -> Result<(), MaterializationError> {
        let result = self
            .db
            .delete_item()
            .table_name(&self.table)
            .key(ROOT_ID, s(&key.root_id))
            .key(TARGET_KEY, s(&key.target_key))
            .condition_expression("generation = :generation AND #state = :closed")
            .expression_attribute_names("#state", STATE)
            .expression_attribute_values(":generation", s(generation))
            .expression_attribute_values(":closed", s(CLOSED))
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(error) if conditional_failure(&error) => match self.read(key).await? {
                None => Ok(()),
                Some(_) => Err(MaterializationError::ReservationLost { cleanup: None }),
            },
            Err(error) => Err(storage_error("purge terminal target", &error)),
        }
    }
}

#[async_trait]
impl TargetReservations for DynamoTargetRegistry {
    async fn acquire(
        &self,
        request: &AcquireTarget,
    ) -> Result<AcquireOutcome, MaterializationError> {
        let lease = request.lease()?;
        let put = Put::builder()
            .table_name(&self.table)
            .set_item(Some(materializing_item(&lease, request.now_ms)))
            .condition_expression("attribute_not_exists(root_id)")
            .build()
            .map_err(|error| {
                MaterializationError::Storage(format!("reserve target build: {error}"))
            })?;
        let capacity = capacity_add_update(
            &self.table,
            lease.spec.materialized_mib,
            self.max_materialized_mib,
            request.now_ms,
        )?;
        let result = self
            .db
            .transact_write_items()
            .transact_items(TransactWriteItem::builder().put(put).build())
            .transact_items(TransactWriteItem::builder().update(capacity).build())
            .send()
            .await;
        match result {
            Ok(_) => return Ok(AcquireOutcome::Acquired(lease)),
            Err(error) if !transaction_cancelled(&error) => {
                return Err(storage_error("reserve target", &error));
            }
            Err(_) => {}
        }

        // A failed conditional Put can race a transition. Bound retries and always use a strong
        // read; callers retry temporary races rather than broadening routing.
        for _ in 0..4 {
            let Some(current) = self.read(&request.key).await? else {
                let reserved_mib = self.reserved_capacity().await?;
                if let Some(error) = plane_capacity_error(
                    reserved_mib,
                    lease.spec.materialized_mib,
                    self.max_materialized_mib,
                ) {
                    return Err(error);
                }
                // TransactionCanceled also covers transaction conflicts and other non-capacity
                // races. Never report those as quota exhaustion merely because the target Put
                // did not commit.
                return Err(MaterializationError::Storage(
                    "target reservation transaction was cancelled despite available capacity"
                        .into(),
                ));
            };
            current.validate()?;
            if current.spec_digest != lease.spec_digest {
                return Err(MaterializationError::SpecConflict);
            }
            if request.generation_is_fenced
                && current.generation != lease.generation
                && !(request.replace_after_loss
                    && matches!(
                        &current.state,
                        DurableTargetState::Closed {
                            disposition: Disposition::Lost,
                            ..
                        }
                    ))
            {
                return Err(MaterializationError::SpecConflict);
            }
            match current.state.clone() {
                DurableTargetState::Installed { .. } => {
                    return Ok(AcquireOutcome::Installed(
                        current.installed().expect("installed state projects"),
                    ));
                }
                DurableTargetState::Materializing {
                    lease_expires_at_ms,
                    ..
                } if lease_expires_at_ms <= request.now_ms => {
                    if self
                        .replace_expired_materialization(&current, request, &lease)
                        .await?
                    {
                        return Ok(AcquireOutcome::Acquired(lease));
                    }
                }
                DurableTargetState::Materializing {
                    target_expires_at_ms,
                    lease_expires_at_ms,
                    ..
                } if target_expires_at_ms <= request.now_ms => {
                    return Ok(AcquireOutcome::Pending {
                        generation: current.generation,
                        retry_after_ms: materialization_poll_after(
                            lease_expires_at_ms,
                            request.now_ms,
                        ),
                    });
                }
                DurableTargetState::Materializing {
                    attempt_expires_at_ms,
                    ..
                } if attempt_expires_at_ms > request.now_ms => {
                    return Ok(AcquireOutcome::Pending {
                        generation: current.generation,
                        retry_after_ms: materialization_poll_after(
                            attempt_expires_at_ms,
                            request.now_ms,
                        ),
                    });
                }
                DurableTargetState::Materializing { .. } => {
                    if let Some(recovered) = self.take_expired_attempt(&current, request).await? {
                        return Ok(AcquireOutcome::Acquired(recovered));
                    }
                }
                DurableTargetState::Closed {
                    disposition: Disposition::Lost,
                    ..
                } if request.replace_after_loss => {
                    if self
                        .replace_gone_environment(&current, request, &lease)
                        .await?
                    {
                        return Ok(AcquireOutcome::Acquired(lease));
                    }
                    // A transaction cancellation also represents a target-row race, so never
                    // infer capacity from that response alone. Confirm both the exact Gone row
                    // and the current plane counter with strong reads before rating the failure.
                    let unchanged = self.read(&request.key).await?.is_some_and(|latest| {
                        latest.generation == current.generation
                            && latest.spec_digest == current.spec_digest
                            && matches!(
                                latest.state,
                                DurableTargetState::Closed {
                                    disposition: Disposition::Lost,
                                    ..
                                }
                            )
                    });
                    if unchanged {
                        let reserved_mib = self.reserved_capacity().await?;
                        if let Some(error) = plane_capacity_error(
                            reserved_mib,
                            lease.spec.materialized_mib,
                            self.max_materialized_mib,
                        ) {
                            return Err(error);
                        }
                    }
                }
                DurableTargetState::Closed {
                    disposition: Disposition::Lost,
                    ..
                } => return Ok(AcquireOutcome::Gone),
                DurableTargetState::Closed {
                    disposition: Disposition::Terminated,
                    ..
                } => {
                    return Ok(AcquireOutcome::Terminated);
                }
            }
        }
        Err(MaterializationError::Storage(
            "target changed during bounded reservation attempts".into(),
        ))
    }

    async fn install(
        &self,
        lease: &MaterializationLease,
        target: &environment_core::materialization::PhysicalTarget,
        now_ms: u64,
    ) -> Result<InstallOutcome, MaterializationError> {
        let result = self
            .db
            .update_item()
            .table_name(&self.table)
            .key(ROOT_ID, AttributeValue::S(lease.key.root_id.clone()))
            .key(TARGET_KEY, AttributeValue::S(lease.key.target_key.clone()))
            .condition_expression(
                "#state = :materializing AND reservation_id = :reservation \
                 AND generation = :generation AND spec_digest = :spec_digest",
            )
            .update_expression(
                "SET #state = :installed, target_ref = :target_ref, \
                 generation = :target_generation, control_token = :control_token, \
                 installed_at_ms = :now, updated_at_ms = :now \
                 REMOVE reservation_id, lease_expires_at_ms, reason, disposition, \
                 at_ms, expires_at_s, launch_request, attempt_id, \
                 attempt_expires_at_ms",
            )
            .expression_attribute_names("#state", STATE)
            .expression_attribute_values(":materializing", s(MATERIALIZING))
            .expression_attribute_values(":installed", s(INSTALLED))
            .expression_attribute_values(":reservation", s(&lease.reservation_id))
            .expression_attribute_values(":generation", s(&lease.generation))
            .expression_attribute_values(":target_generation", s(&target.generation))
            .expression_attribute_values(
                ":control_token",
                AttributeValue::B(Blob::new(target.control_token.expose().as_bytes())),
            )
            .expression_attribute_values(":spec_digest", s(&lease.spec_digest))
            .expression_attribute_values(":target_ref", s(&target.target_ref))
            .expression_attribute_values(":now", n(now_ms))
            .return_values(ReturnValue::AllNew)
            .send()
            .await;
        match result {
            Ok(output) => {
                let attrs = output.attributes().ok_or_else(|| {
                    MaterializationError::Corrupt("install returned no target record".into())
                })?;
                let record = parse_record(attrs)?;
                Ok(InstallOutcome::Installed(record.installed().ok_or_else(
                    || {
                        MaterializationError::Corrupt(
                            "install returned a non-installed target".into(),
                        )
                    },
                )?))
            }
            Err(error) if conditional_failure(&error) => {
                let existing = self.read(&lease.key).await?;
                let exact = existing
                    .and_then(|record| record.installed())
                    .filter(|installed| {
                        installed.target_ref == target.target_ref
                            && installed.generation == target.generation
                            && installed.control_token == target.control_token
                            && installed.spec_digest == lease.spec_digest
                    });
                Ok(exact.map_or(InstallOutcome::ReservationLost, InstallOutcome::Installed))
            }
            Err(error) => Err(storage_error("install target", &error)),
        }
    }

    async fn expire_lease(
        &self,
        lease: &MaterializationLease,
        now_ms: u64,
    ) -> Result<(), MaterializationError> {
        // This method is called only after the launcher attests that no target exists (or by the
        // lease reconciler after the provider confirms an orphan is gone). Delete and refund are
        // one transaction, so a crash cannot create allocatable capacity without removing the
        // exact reservation or remove the reservation without refunding it.
        let delete = Delete::builder()
            .table_name(&self.table)
            .key(ROOT_ID, AttributeValue::S(lease.key.root_id.clone()))
            .key(TARGET_KEY, AttributeValue::S(lease.key.target_key.clone()))
            .condition_expression(
                "#state = :materializing AND reservation_id = :reservation \
                 AND generation = :generation AND attempt_id = :attempt_id",
            )
            .expression_attribute_names("#state", STATE)
            .expression_attribute_values(":materializing", s(MATERIALIZING))
            .expression_attribute_values(":reservation", s(&lease.reservation_id))
            .expression_attribute_values(":generation", s(&lease.generation))
            .expression_attribute_values(":attempt_id", s(&lease.attempt_id))
            .build()
            .map_err(|error| {
                MaterializationError::Storage(format!("release lease build: {error}"))
            })?;
        let capacity = capacity_subtract_update(&self.table, lease.spec.materialized_mib, now_ms)?;
        let result = self
            .db
            .transact_write_items()
            .transact_items(TransactWriteItem::builder().delete(delete).build())
            .transact_items(TransactWriteItem::builder().update(capacity).build())
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(error) if transaction_cancelled(&error) => {
                let current = self.read(&lease.key).await?;
                match current {
                    None => Ok(()),
                    Some(record)
                        if record.generation != lease.generation
                            || !matches!(
                                record.state,
                                DurableTargetState::Materializing {
                                    ref reservation_id,
                                    ref attempt_id,
                                    ..
                                } if reservation_id == &lease.reservation_id
                                    && attempt_id == &lease.attempt_id
                            ) =>
                    {
                        Ok(())
                    }
                    Some(_) => Err(MaterializationError::Storage(
                        "exact materialization lease could not refund capacity".into(),
                    )),
                }
            }
            Err(error) => Err(storage_error("expire target lease", &error)),
        }
    }
}

#[async_trait]
impl TargetDirectory for DynamoTargetRegistry {
    async fn get(
        &self,
        key: &TargetKey,
    ) -> Result<Option<DurableTargetRecord>, MaterializationError> {
        key.validate()?;
        self.read(key).await
    }

    async fn mark_closed(
        &self,
        target: &InstalledTarget,
        disposition: Disposition,
        reason: &str,
        now_ms: u64,
    ) -> Result<(), MaterializationError> {
        transition_closed(self, target, disposition, reason, now_ms).await
    }

    async fn list_root(
        &self,
        root_id: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<TargetPage, MaterializationError> {
        TargetKey::for_environment(root_id, "validation")?.validate()?;
        let limit = limit.clamp(1, MAX_TARGET_PAGE);
        let start_key = cursor.map(|cursor| {
            HashMap::from([(ROOT_ID.into(), s(root_id)), (TARGET_KEY.into(), s(cursor))])
        });
        let output = self
            .db
            .query()
            .table_name(&self.table)
            .key_condition_expression(
                "root_id = :root_id AND begins_with(target_key, :target_prefix)",
            )
            .expression_attribute_values(":root_id", s(root_id))
            .expression_attribute_values(":target_prefix", s(TARGET_KEY_PREFIX))
            .consistent_read(true)
            .limit(limit as i32)
            .set_exclusive_start_key(start_key)
            .send()
            .await
            .map_err(|error| storage_error("list root targets", &error))?;
        let items = output
            .items()
            .iter()
            .map(parse_record)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = output
            .last_evaluated_key()
            .and_then(|key| key.get(TARGET_KEY))
            .and_then(|value| value.as_s().ok())
            .cloned();
        Ok(TargetPage { items, next_cursor })
    }
}

fn disposition_value(disposition: Disposition) -> &'static str {
    match disposition {
        Disposition::Lost => DISPOSITION_LOST,
        Disposition::Terminated => DISPOSITION_TERMINATED,
    }
}

async fn transition_closed(
    registry: &DynamoTargetRegistry,
    target: &InstalledTarget,
    disposition: Disposition,
    reason: &str,
    now_ms: u64,
) -> Result<(), MaterializationError> {
    target.validate()?;
    if reason.is_empty() || reason.len() > MAX_REASON_BYTES {
        return Err(MaterializationError::InvalidIdentity("reason"));
    }
    let update = Update::builder()
        .table_name(&registry.table)
        .key(ROOT_ID, AttributeValue::S(target.key.root_id.clone()))
        .key(TARGET_KEY, AttributeValue::S(target.key.target_key.clone()))
        .condition_expression(
            "#state = :installed AND target_ref = :target_ref \
             AND generation = :generation AND spec_digest = :spec_digest",
        )
        .update_expression(
            "SET #state = :new_state, disposition = :disposition, reason = :reason, \
             at_ms = :now, updated_at_ms = :now \
             REMOVE target_ref, control_token, installed_at_ms, expires_at_ms, expires_at_s",
        )
        .expression_attribute_names("#state", STATE)
        .expression_attribute_values(":installed", s(INSTALLED))
        .expression_attribute_values(":new_state", s(CLOSED))
        .expression_attribute_values(":disposition", s(disposition_value(disposition)))
        .expression_attribute_values(":target_ref", s(&target.target_ref))
        .expression_attribute_values(":generation", s(&target.generation))
        .expression_attribute_values(":spec_digest", s(&target.spec_digest))
        .expression_attribute_values(":reason", s(reason))
        .expression_attribute_values(":now", n(now_ms));
    let update = update.build().map_err(|error| {
        MaterializationError::Storage(format!("transition target build: {error}"))
    })?;
    let capacity = capacity_subtract_update(&registry.table, target.spec.materialized_mib, now_ms)?;
    let transaction = registry
        .db
        .transact_write_items()
        .transact_items(TransactWriteItem::builder().update(update).build())
        .transact_items(TransactWriteItem::builder().update(capacity).build());
    match transaction.send().await {
        Ok(_) => Ok(()),
        Err(error) if transaction_cancelled(&error) => {
            let current = registry.read(&target.key).await?;
            match current {
                Some(record)
                    if record.generation == target.generation
                        && matches!(
                            &record.state,
                            DurableTargetState::Closed { disposition: stored, .. }
                                if *stored == disposition
                        ) =>
                {
                    Ok(())
                }
                _ => Err(MaterializationError::ReservationLost { cleanup: None }),
            }
        }
        Err(error) => Err(storage_error("transition target", &error)),
    }
}

fn materializing_item(
    lease: &MaterializationLease,
    now_ms: u64,
) -> HashMap<String, AttributeValue> {
    HashMap::from([
        (ROOT_ID.into(), s(&lease.key.root_id)),
        (TARGET_KEY.into(), s(&lease.key.target_key)),
        (STATE.into(), s(MATERIALIZING)),
        ("spec_digest".into(), s(&lease.spec_digest)),
        (
            "connector".into(),
            s(match lease.spec.connector {
                ConnectorClass::None => "none",
                ConnectorClass::Public => "public",
                ConnectorClass::Allowlist => "allowlist",
            }),
        ),
        ("image_identity".into(), s(&lease.spec.image_identity)),
        ("resource_class".into(), s(&lease.spec.resource_class)),
        ("materialized_mib".into(), n(lease.spec.materialized_mib)),
        (
            "resource_policy_digest".into(),
            s(&lease.spec.resource_policy_digest),
        ),
        (
            "network_policy_digest".into(),
            s(&lease.spec.network_policy_digest),
        ),
        ("reservation_id".into(), s(&lease.reservation_id)),
        ("generation".into(), s(&lease.generation)),
        (
            LAUNCH_REQUEST.into(),
            AttributeValue::B(Blob::new(lease.launch_request.expose().as_bytes())),
        ),
        (ATTEMPT_ID.into(), s(&lease.attempt_id)),
        (ATTEMPT_EXPIRES_AT_MS.into(), n(lease.attempt_expires_at_ms)),
        ("expires_at_ms".into(), n(lease.target_expires_at_ms)),
        ("lease_expires_at_ms".into(), n(lease.lease_expires_at_ms)),
        ("updated_at_ms".into(), n(now_ms)),
    ])
}

fn parse_record(
    attrs: &HashMap<String, AttributeValue>,
) -> Result<DurableTargetRecord, MaterializationError> {
    let string = |name: &'static str| -> Result<String, MaterializationError> {
        attrs
            .get(name)
            .and_then(|value| value.as_s().ok())
            .cloned()
            .ok_or_else(|| corrupt(format!("missing string {name}")))
    };
    let number = |name: &'static str| -> Result<u64, MaterializationError> {
        attrs
            .get(name)
            .and_then(|value| value.as_n().ok())
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| corrupt(format!("missing numeric {name}")))
    };
    let key = TargetKey {
        root_id: string(ROOT_ID)?,
        target_key: string(TARGET_KEY)?,
    };
    let spec = TargetSpec::new(
        match string("connector")?.as_str() {
            "none" => ConnectorClass::None,
            "public" => ConnectorClass::Public,
            "allowlist" => ConnectorClass::Allowlist,
            other => return Err(corrupt(format!("unknown connector {other}"))),
        },
        string("image_identity")?,
        string("resource_class")?,
        number("materialized_mib")?,
        string("resource_policy_digest")?,
        string("network_policy_digest")?,
    )?;
    let spec_digest = string("spec_digest")?;
    let generation = string("generation")?;
    let state = match string(STATE)?.as_str() {
        MATERIALIZING => DurableTargetState::Materializing {
            reservation_id: string("reservation_id")?,
            launch_request: attrs
                .get(LAUNCH_REQUEST)
                .and_then(|value| value.as_b().ok())
                .map(|value| value.as_ref().to_vec())
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .ok_or_else(|| corrupt("missing binary launch_request"))
                .and_then(|value| DurableLaunchRequest::new(value).map_err(Into::into))?,
            attempt_id: string(ATTEMPT_ID)?,
            attempt_expires_at_ms: number(ATTEMPT_EXPIRES_AT_MS)?,
            target_expires_at_ms: number("expires_at_ms")?,
            lease_expires_at_ms: number("lease_expires_at_ms")?,
        },
        INSTALLED => DurableTargetState::Installed {
            target_ref: string("target_ref")?,
            control_token: attrs
                .get(CONTROL_TOKEN)
                .and_then(|value| value.as_b().ok())
                .map(|value| value.as_ref().to_vec())
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .ok_or_else(|| corrupt("missing binary control_token"))
                .and_then(|value| ControlToken::new(value).map_err(Into::into))?,
            installed_at_ms: number("installed_at_ms")?,
            expires_at_ms: number("expires_at_ms")?,
        },
        CLOSED => DurableTargetState::Closed {
            disposition: match string("disposition")?.as_str() {
                DISPOSITION_LOST => Disposition::Lost,
                DISPOSITION_TERMINATED => Disposition::Terminated,
                other => return Err(corrupt(format!("unknown disposition {other}"))),
            },
            reason: string("reason")?,
            at_ms: number("at_ms")?,
        },
        other => return Err(corrupt(format!("unknown state {other}"))),
    };
    let record = DurableTargetRecord {
        key,
        spec,
        spec_digest,
        generation,
        state,
        updated_at_ms: number("updated_at_ms")?,
    };
    record.validate()?;
    Ok(record)
}

fn materializing_reservation(record: &DurableTargetRecord) -> Result<&str, MaterializationError> {
    match &record.state {
        DurableTargetState::Materializing { reservation_id, .. } => Ok(reservation_id),
        _ => Err(corrupt("expected materializing target")),
    }
}

fn conditional_result<E: ProvideErrorMetadata, R>(
    operation: &str,
    result: Result<impl Sized, SdkError<E, R>>,
) -> Result<bool, MaterializationError> {
    match result {
        Ok(_) => Ok(true),
        Err(error) if conditional_failure(&error) => Ok(false),
        Err(error) => Err(storage_error(operation, &error)),
    }
}

fn transaction_cancelled<E: ProvideErrorMetadata, R>(error: &SdkError<E, R>) -> bool {
    matches!(
        error,
        SdkError::ServiceError(service)
            if service.err().code() == Some("TransactionCanceledException")
    )
}

fn transaction_result<E: ProvideErrorMetadata, R>(
    operation: &str,
    result: Result<impl Sized, SdkError<E, R>>,
) -> Result<bool, MaterializationError> {
    match result {
        Ok(_) => Ok(true),
        Err(error) if transaction_cancelled(&error) => Ok(false),
        Err(error) => Err(storage_error(operation, &error)),
    }
}

fn capacity_key() -> HashMap<String, AttributeValue> {
    HashMap::from([
        (ROOT_ID.into(), s(CAPACITY_ROOT_ID)),
        (TARGET_KEY.into(), s(CAPACITY_TARGET_KEY)),
    ])
}

fn plane_capacity_error(
    reserved_mib: u64,
    requested_mib: u64,
    max_materialized_mib: u64,
) -> Option<MaterializationError> {
    let available_mib = max_materialized_mib.saturating_sub(reserved_mib);
    (requested_mib > available_mib).then(|| MaterializationError::Capacity {
        scope: "plane_materialized_memory_mib".into(),
        retry_after_ms: 1_000,
        message: format!(
            "{requested_mib} MiB target exceeds remaining plane allocation of {available_mib} MiB"
        ),
    })
}

fn capacity_add_update(
    table: &str,
    materialized_mib: u64,
    max_materialized_mib: u64,
    now_ms: u64,
) -> Result<Update, MaterializationError> {
    let remaining = max_materialized_mib
        .checked_sub(materialized_mib)
        .ok_or_else(|| MaterializationError::Capacity {
            scope: "plane_materialized_memory_mib".into(),
            retry_after_ms: 1_000,
            message: format!(
                "{materialized_mib} MiB target exceeds the {max_materialized_mib} MiB plane allocation"
            ),
        })?;
    Update::builder()
        .table_name(table)
        .set_key(Some(capacity_key()))
        .condition_expression("attribute_not_exists(reserved_mib) OR reserved_mib <= :remaining")
        .update_expression(
            "SET reserved_mib = if_not_exists(reserved_mib, :zero) + :mib, updated_at_ms = :now",
        )
        .expression_attribute_values(":remaining", n(remaining))
        .expression_attribute_values(":zero", n(0))
        .expression_attribute_values(":mib", n(materialized_mib))
        .expression_attribute_values(":now", n(now_ms))
        .build()
        .map_err(|error| MaterializationError::Storage(format!("capacity add build: {error}")))
}

fn capacity_subtract_update(
    table: &str,
    materialized_mib: u64,
    now_ms: u64,
) -> Result<Update, MaterializationError> {
    Update::builder()
        .table_name(table)
        .set_key(Some(capacity_key()))
        .condition_expression("reserved_mib >= :mib")
        .update_expression("SET reserved_mib = reserved_mib - :mib, updated_at_ms = :now")
        .expression_attribute_values(":mib", n(materialized_mib))
        .expression_attribute_values(":now", n(now_ms))
        .build()
        .map_err(|error| MaterializationError::Storage(format!("capacity subtract build: {error}")))
}

fn storage_error<E: ProvideErrorMetadata, R>(
    operation: &str,
    error: &SdkError<E, R>,
) -> MaterializationError {
    MaterializationError::Storage(crate::dynamo::storage_detail(operation, error))
}

fn corrupt(message: impl Into<String>) -> MaterializationError {
    MaterializationError::Corrupt(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease() -> MaterializationLease {
        AcquireTarget {
            key: TargetKey::for_environment("root-1", "binding-1").unwrap(),
            spec: TargetSpec::new(
                ConnectorClass::Allowlist,
                "image-1",
                "microvm-1gb",
                1024,
                "a".repeat(64),
                "b".repeat(64),
            )
            .unwrap(),
            reservation_id: "reservation-1".into(),
            generation: "generation-1".into(),
            launch_request: environment_core::materialization::DurableLaunchRequest::new(
                "sealed-launch-request",
            )
            .unwrap(),
            attempt_id: "attempt-1".into(),
            attempt_duration_ms: 100,
            generation_is_fenced: true,
            now_ms: 10,
            lease_duration_ms: 1_000,
            target_lifetime_ms: 900,
            replace_after_loss: true,
        }
        .lease()
        .unwrap()
    }

    #[test]
    fn materializing_item_round_trips_with_sensitive_launch_bytes_debug_redacted() {
        let lease = lease();
        let item = materializing_item(&lease, 10);
        let parsed = parse_record(&item).unwrap();
        assert_eq!(parsed.key, lease.key);
        assert_eq!(parsed.spec, lease.spec);
        assert_eq!(parsed.generation, lease.generation);
        assert!(matches!(
            parsed.state,
            DurableTargetState::Materializing { .. }
        ));
        assert_eq!(
            item.get(LAUNCH_REQUEST).and_then(|value| value.as_b().ok()),
            Some(&Blob::new("sealed-launch-request"))
        );
        assert!(!format!("{parsed:?}").contains("sealed-launch-request"));
        for forbidden in ["capability", "session_token", "endpoint", "auth_token"] {
            assert!(!item.contains_key(forbidden));
        }
    }

    #[test]
    fn installed_and_terminal_records_parse_exactly() {
        let lease = lease();
        let mut installed = materializing_item(&lease, 10);
        installed.insert(STATE.into(), s(INSTALLED));
        installed.remove("reservation_id");
        installed.remove(LAUNCH_REQUEST);
        installed.remove(ATTEMPT_ID);
        installed.remove(ATTEMPT_EXPIRES_AT_MS);
        installed.remove("lease_expires_at_ms");
        installed.insert("target_ref".into(), s("mvm-1"));
        installed.insert(
            CONTROL_TOKEN.into(),
            AttributeValue::B(Blob::new(format!("control-{}", "c".repeat(64)))),
        );
        installed.insert("installed_at_ms".into(), n(20));
        let parsed = parse_record(&installed).unwrap();
        assert!(parsed.installed().is_some());
        assert!(!format!("{parsed:?}").contains(&format!("control-{}", "c".repeat(64))));

        installed.insert(STATE.into(), s(CLOSED));
        installed.remove("target_ref");
        installed.remove(CONTROL_TOKEN);
        installed.remove("installed_at_ms");
        installed.remove("expires_at_ms");
        installed.insert("disposition".into(), s(DISPOSITION_TERMINATED));
        installed.insert("reason".into(), s("session deleted"));
        installed.insert("at_ms".into(), n(30));
        assert!(matches!(
            parse_record(&installed).unwrap().state,
            DurableTargetState::Closed {
                disposition: Disposition::Terminated,
                ..
            }
        ));
    }

    #[test]
    fn plane_capacity_classification_reports_the_actual_remaining_allocation() {
        assert!(plane_capacity_error(4_096, 1_024, 5_120).is_none());
        let MaterializationError::Capacity {
            scope,
            retry_after_ms,
            message,
        } = plane_capacity_error(5_120, 1_024, 5_120).unwrap()
        else {
            panic!("full plane must return typed capacity");
        };
        assert_eq!(scope, "plane_materialized_memory_mib");
        assert_eq!(retry_after_ms, 1_000);
        assert!(message.contains("remaining plane allocation of 0 MiB"));
    }
}
