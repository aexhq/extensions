//! Reservation, deduplication and terminal-result retention for effectful operations.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A retained operation. Request and terminal digests are supplied by Brain's exact contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRecord {
    pub operation_id: String,
    pub request_digest: String,
    /// Bytes admitted before the effect starts. Once terminal this equals the retained payload.
    pub retention_reservation_bytes: usize,
    pub state: OperationState,
    pub cancel_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum OperationState {
    Accepted,
    Running,
    Terminal {
        terminal_digest: String,
        /// Exact encoded terminal observation. The adapter decodes it using Brain's type.
        payload: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reservation {
    New,
    Existing,
}

/// A bounded in-generation operation registry.
///
/// It never evicts an unacknowledged terminal result. Capacity pressure rejects new work before
/// its effect starts. Loss of the physical generation turns outstanding work into `unknown` at
/// the adapter boundary; it does not make replay safe.
#[derive(Debug, Serialize, Deserialize)]
pub struct OperationRegistry {
    records: HashMap<String, OperationRecord>,
    max_records: usize,
    max_terminal_bytes: usize,
    terminal_bytes: usize,
    pending_reserved_bytes: usize,
}

impl OperationRegistry {
    #[must_use]
    pub fn new(max_records: usize, max_terminal_bytes: usize) -> Self {
        Self {
            records: HashMap::new(),
            max_records,
            max_terminal_bytes,
            terminal_bytes: 0,
            pending_reserved_bytes: 0,
        }
    }

    pub fn reserve(
        &mut self,
        operation_id: &str,
        request_digest: &str,
        retention_reservation_bytes: usize,
    ) -> Result<Reservation, OperationError> {
        validate_identifier(operation_id, "operation_id")?;
        validate_digest(request_digest, "request_digest")?;
        if let Some(existing) = self.records.get(operation_id) {
            return if existing.request_digest == request_digest {
                Ok(Reservation::Existing)
            } else {
                Err(OperationError::IdempotencyConflict)
            };
        }
        let retained_after_reservation = self
            .terminal_bytes
            .checked_add(self.pending_reserved_bytes)
            .and_then(|bytes| bytes.checked_add(retention_reservation_bytes))
            .ok_or(OperationError::Capacity)?;
        if retention_reservation_bytes == 0
            || self.records.len() >= self.max_records
            || retained_after_reservation > self.max_terminal_bytes
        {
            return Err(OperationError::Capacity);
        }
        self.pending_reserved_bytes = self
            .pending_reserved_bytes
            .checked_add(retention_reservation_bytes)
            .ok_or(OperationError::Capacity)?;
        self.records.insert(
            operation_id.to_owned(),
            OperationRecord {
                operation_id: operation_id.to_owned(),
                request_digest: request_digest.to_owned(),
                retention_reservation_bytes,
                state: OperationState::Accepted,
                cancel_requested: false,
            },
        );
        Ok(Reservation::New)
    }

    pub fn mark_running(&mut self, operation_id: &str) -> Result<(), OperationError> {
        let record = self
            .records
            .get_mut(operation_id)
            .ok_or(OperationError::Unknown)?;
        match record.state {
            OperationState::Accepted => {
                record.state = OperationState::Running;
                Ok(())
            }
            OperationState::Running => Ok(()),
            OperationState::Terminal { .. } => Err(OperationError::AlreadyTerminal),
        }
    }

    /// Installs terminal data once. An exact repeated report is idempotent.
    pub fn complete(
        &mut self,
        operation_id: &str,
        terminal_digest: &str,
        payload: Vec<u8>,
    ) -> Result<(), OperationError> {
        validate_digest(terminal_digest, "terminal_digest")?;
        let record = self
            .records
            .get_mut(operation_id)
            .ok_or(OperationError::Unknown)?;
        match &record.state {
            OperationState::Terminal {
                terminal_digest: existing_digest,
                payload: existing_payload,
            } if existing_digest == terminal_digest && existing_payload == &payload => Ok(()),
            OperationState::Terminal { .. } => Err(OperationError::TerminalConflict),
            OperationState::Accepted | OperationState::Running => {
                if payload.len() > record.retention_reservation_bytes {
                    return Err(OperationError::TerminalCapacity);
                }
                self.pending_reserved_bytes = self
                    .pending_reserved_bytes
                    .saturating_sub(record.retention_reservation_bytes);
                self.terminal_bytes = self
                    .terminal_bytes
                    .checked_add(payload.len())
                    .ok_or(OperationError::TerminalCapacity)?;
                record.retention_reservation_bytes = payload.len();
                record.state = OperationState::Terminal {
                    terminal_digest: terminal_digest.to_owned(),
                    payload,
                };
                Ok(())
            }
        }
    }

    /// Records intent to cancel; it never claims the underlying effect has stopped.
    pub fn request_cancel(&mut self, operation_id: &str) -> Result<bool, OperationError> {
        let record = self
            .records
            .get_mut(operation_id)
            .ok_or(OperationError::Unknown)?;
        if matches!(record.state, OperationState::Terminal { .. }) {
            return Ok(false);
        }
        let first = !record.cancel_requested;
        record.cancel_requested = true;
        Ok(first)
    }

    #[must_use]
    pub fn observe(&self, operation_id: &str) -> Option<&OperationRecord> {
        self.records.get(operation_id)
    }

    /// Forgets an exact terminal result only after Brain commits and acknowledges its digest.
    pub fn validate_terminal_ack(
        &self,
        operation_id: &str,
        terminal_digest: &str,
    ) -> Result<(), OperationError> {
        validate_identifier(operation_id, "operation_id")?;
        validate_digest(terminal_digest, "terminal_digest")?;
        let record = self
            .records
            .get(operation_id)
            .ok_or(OperationError::Unknown)?;
        match &record.state {
            OperationState::Terminal {
                terminal_digest: existing,
                ..
            } if existing == terminal_digest => Ok(()),
            OperationState::Terminal { .. } => Err(OperationError::TerminalDigestMismatch),
            OperationState::Accepted | OperationState::Running => Err(OperationError::NotTerminal),
        }
    }

    /// Forgets an exact terminal result only after its durable acknowledgement fence exists.
    pub fn acknowledge_terminal(
        &mut self,
        operation_id: &str,
        terminal_digest: &str,
    ) -> Result<(), OperationError> {
        self.validate_terminal_ack(operation_id, terminal_digest)?;
        let bytes = match &self.records[operation_id].state {
            OperationState::Terminal { payload, .. } => payload.len(),
            OperationState::Accepted | OperationState::Running => unreachable!(),
        };
        self.records.remove(operation_id);
        self.terminal_bytes = self.terminal_bytes.saturating_sub(bytes);
        Ok(())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[must_use]
    pub fn terminal_bytes(&self) -> usize {
        self.terminal_bytes
    }

    #[must_use]
    pub fn pending_reserved_bytes(&self) -> usize {
        self.pending_reserved_bytes
    }
}

fn validate_identifier(value: &str, field: &'static str) -> Result<(), OperationError> {
    environment_policy::identity::validate_identifier(value, field)
        .map_err(|error| OperationError::InvalidIdentity(error.field))
}

fn validate_digest(value: &str, field: &'static str) -> Result<(), OperationError> {
    environment_policy::identity::validate_digest(value, field)
        .map_err(|error| OperationError::InvalidIdentity(error.field))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OperationError {
    #[error("{0} does not satisfy the canonical Environment identity or digest grammar")]
    InvalidIdentity(&'static str),
    #[error("operation id is already reserved for a different request digest")]
    IdempotencyConflict,
    #[error("operation retention capacity is full")]
    Capacity,
    #[error("operation is unknown")]
    Unknown,
    #[error("operation is already terminal")]
    AlreadyTerminal,
    #[error("a different terminal observation is already retained")]
    TerminalConflict,
    #[error("terminal observation exceeds retention capacity")]
    TerminalCapacity,
    #[error("operation is not terminal")]
    NotTerminal,
    #[error("terminal digest does not match the retained observation")]
    TerminalDigestMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_reservation_replays_and_a_new_digest_conflicts() {
        let mut registry = OperationRegistry::new(4, 1024);
        let request_a = "a".repeat(64);
        let request_b = "b".repeat(64);
        assert_eq!(
            registry.reserve("op-1", &request_a, 512),
            Ok(Reservation::New)
        );
        assert_eq!(
            registry.reserve("op-1", &request_a, 512),
            Ok(Reservation::Existing)
        );
        assert_eq!(
            registry.reserve("op-1", &request_b, 512),
            Err(OperationError::IdempotencyConflict)
        );
    }

    #[test]
    fn terminal_result_survives_until_exact_acknowledgement() {
        let mut registry = OperationRegistry::new(4, 1024);
        registry.reserve("op-1", &"a".repeat(64), 1024).unwrap();
        registry.mark_running("op-1").unwrap();
        registry
            .complete("op-1", &"c".repeat(64), b"terminal bytes".to_vec())
            .unwrap();
        registry
            .complete("op-1", &"c".repeat(64), b"terminal bytes".to_vec())
            .unwrap();
        assert_eq!(registry.terminal_bytes(), 14);
        assert_eq!(
            registry.acknowledge_terminal("op-1", &"d".repeat(64)),
            Err(OperationError::TerminalDigestMismatch)
        );
        assert!(registry.observe("op-1").is_some());
        registry
            .acknowledge_terminal("op-1", &"c".repeat(64))
            .unwrap();
        assert!(registry.is_empty());
        assert_eq!(registry.terminal_bytes(), 0);
    }

    #[test]
    fn capacity_rejects_before_reserving_a_new_effect() {
        let mut registry = OperationRegistry::new(1, 4);
        registry.reserve("op-1", &"a".repeat(64), 4).unwrap();
        assert_eq!(
            registry.reserve("op-2", &"b".repeat(64), 1),
            Err(OperationError::Capacity)
        );
        assert_eq!(
            registry.complete("op-1", &"c".repeat(64), b"12345".to_vec()),
            Err(OperationError::TerminalCapacity)
        );
        assert_eq!(registry.terminal_bytes(), 0);
        assert_eq!(registry.pending_reserved_bytes(), 4);
        registry
            .complete("op-1", &"c".repeat(64), b"1234".to_vec())
            .unwrap();
        assert_eq!(registry.terminal_bytes(), 4);
        assert_eq!(
            registry.reserve("op-2", &"b".repeat(64), 1),
            Err(OperationError::Capacity)
        );
    }

    #[test]
    fn cancellation_is_a_request_not_a_terminal_claim() {
        let mut registry = OperationRegistry::new(1, 4);
        registry.reserve("op-1", &"a".repeat(64), 4).unwrap();
        assert_eq!(registry.request_cancel("op-1"), Ok(true));
        assert_eq!(registry.request_cancel("op-1"), Ok(false));
        assert!(matches!(
            registry.observe("op-1").unwrap().state,
            OperationState::Accepted
        ));
    }
}
