//! Idempotent interactive stdin writes.

use super::*;

pub(crate) struct StdinBook {
    pub(crate) records: HashMap<String, StdinRecord>,
}

pub(crate) enum StdinRecord {
    InFlight {
        request_digest: Digest,
        /// Wakes exact concurrent retries when the in-flight write settles.
        completed: Arc<Notify>,
    },
    Complete(Box<WriteStdinReceipt>),
}

impl Environment {
    pub async fn write_stdin(
        &self,
        request: WriteStdinRequest,
    ) -> Result<WriteStdinReceipt, EnvironmentError> {
        if write_stdin_request_digest(&request) != request.request_digest {
            return Err(invalid("write_stdin request_digest is not canonical"));
        }
        if request.text.len() > environment_wire::MAX_WRITE_STDIN_BYTES {
            return Err(invalid(
                "stdin text exceeds the atomic 4096-byte pipe-write bound",
            ));
        }
        let target = self
            .fence(&request.target, request.expected_generation.as_str())
            .await?;
        let (control, execution_operation) = {
            let operations = self.operations.book.lock().await;
            let meta = operations
                .metadata
                .get(request.execution_id.as_str())
                .ok_or_else(|| operation_error(OperationError::Unknown))?;
            if meta.operation.operation_id != request.execution_id
                || !canonical_equal(&meta.operation.target, &request.target)?
                || meta.operation.generation.as_str() != target.generation
                || meta.operation.target_ref.as_str() != target.target_ref
                || meta.target.target_ref.as_str() != target.target_ref
                || meta.target.generation.as_str() != target.generation
            {
                return Err(environment_error(
                    EnvironmentErrorCode::OperationConflict,
                    false,
                    "stdin target does not match the reserved sandbox execution",
                ));
            }
            (meta.stdin.clone(), meta.operation.clone())
        };
        // Reserve globally, then release the book lock before touching a potentially full pipe.
        // An exact concurrent retry awaits the in-flight write's completion signal (bounded);
        // unrelated executions never queue behind a hostile shell that refuses to read stdin.
        let wait_deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(MAX_STDIN_REPLAY_WAIT_MS);
        let completed = loop {
            let mut writes = self.stdin.book.lock().await;
            match writes.records.get(request.operation_id.as_str()) {
                Some(StdinRecord::Complete(existing)) => {
                    if existing.request_digest == request.request_digest {
                        let mut receipt = existing.as_ref().clone();
                        receipt.replayed = true;
                        drop(writes);
                        receipt.observation =
                            self.observe_inner(execution_operation.clone(), 0).await?;
                        return Ok(receipt);
                    } else {
                        return Err(stdin_conflict());
                    }
                }
                Some(StdinRecord::InFlight {
                    request_digest,
                    completed,
                }) => {
                    if request_digest != &request.request_digest {
                        return Err(stdin_conflict());
                    }
                    let completed = completed.clone();
                    let notified = completed.notified();
                    tokio::pin!(notified);
                    // Register interest before releasing the book lock, so a completion between
                    // unlock and await cannot be missed.
                    notified.as_mut().enable();
                    drop(writes);
                    if tokio::time::timeout_at(wait_deadline, notified)
                        .await
                        .is_err()
                    {
                        return Err(unavailable(
                            "an exact stdin write is still completing; observe and retry",
                        ));
                    }
                }
                None => {
                    if writes.records.len() >= MAX_RETAINED_STDIN_WRITES {
                        return Err(environment_error(
                            EnvironmentErrorCode::ResourceExhausted,
                            false,
                            "stdin idempotency retention is full for this sandbox generation",
                        ));
                    }
                    let completed = Arc::new(Notify::new());
                    writes.records.insert(
                        request.operation_id.to_string(),
                        StdinRecord::InFlight {
                            request_digest: request.request_digest.clone(),
                            completed: completed.clone(),
                        },
                    );
                    break completed;
                }
            }
        };

        // Empty text without EOF is an observation-only poll. Otherwise the byte bound is
        // PIPE_BUF on supported Linux images, so the one append is all-or-nothing; EOF closes the
        // same pipe only after that append succeeds.
        let accepted = if request.text.is_empty() && !request.eof {
            false
        } else {
            match control {
                Some(control) => {
                    control
                        .send_atomic(request.text.as_bytes(), request.eof)
                        .await
                }
                None => false,
            }
        };
        let observation = self.observe_inner(execution_operation, 0).await?;
        let receipt = WriteStdinReceipt {
            accepted,
            observation,
            operation_id: request.operation_id.clone(),
            replayed: false,
            request_digest: request.request_digest.clone(),
        };
        let mut writes = self.stdin.book.lock().await;
        match writes.records.get(request.operation_id.as_str()) {
            Some(StdinRecord::InFlight { request_digest, .. })
                if request_digest == &request.request_digest =>
            {
                writes.records.insert(
                    request.operation_id.to_string(),
                    StdinRecord::Complete(Box::new(receipt.clone())),
                );
            }
            _ => return Err(stdin_conflict()),
        }
        drop(writes);
        completed.notify_waiters();
        Ok(receipt)
    }
}
