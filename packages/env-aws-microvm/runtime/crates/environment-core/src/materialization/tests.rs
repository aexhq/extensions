use std::sync::atomic::{AtomicUsize, Ordering};

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::*;

struct RejectingLauncher(LaunchError);

#[async_trait]
impl PhysicalTargetLauncher for RejectingLauncher {
    async fn launch(&self, _lease: &MaterializationLease) -> Result<PhysicalTarget, LaunchError> {
        Err(self.0.clone())
    }

    async fn terminate_stale(&self, _target: &PhysicalTarget) -> Result<(), String> {
        Ok(())
    }
}

fn target_spec(connector: ConnectorClass) -> TargetSpec {
    TargetSpec::new(
        connector,
        "image-digest-1",
        "microvm-1gb",
        1024,
        "a".repeat(64),
        "b".repeat(64),
    )
    .unwrap()
}

fn control_token() -> ControlToken {
    ControlToken::new(format!("control-{}", "a".repeat(64))).expect("test control token")
}

#[test]
fn spec_digest_is_canonical_json_independent_of_field_order() {
    let spec = target_spec(ConnectorClass::None);
    let canonical = format!(
        "{{\"connector\":\"none\",\"image_identity\":\"image-digest-1\",\
         \"materialized_mib\":1024,\"network_policy_digest\":\"{}\",\
         \"resource_class\":\"microvm-1gb\",\"resource_policy_digest\":\"{}\"}}",
        "b".repeat(64),
        "a".repeat(64),
    );
    assert_eq!(spec.digest(), hex::encode(Sha256::digest(canonical)));
}

fn physical(target_ref: impl Into<String>, generation: impl Into<String>) -> PhysicalTarget {
    PhysicalTarget::new(target_ref, generation, control_token()).expect("test physical target")
}

fn request(now_ms: u64, reservation: &str, generation: &str) -> AcquireTarget {
    AcquireTarget {
        key: TargetKey::for_default_target("root-1").unwrap(),
        spec: target_spec(ConnectorClass::Allowlist),
        reservation_id: reservation.into(),
        generation: generation.into(),
        launch_request: DurableLaunchRequest::new(format!("launch-{reservation}")).unwrap(),
        attempt_id: format!("attempt-{reservation}"),
        attempt_duration_ms: 100,
        generation_is_fenced: false,
        now_ms,
        lease_duration_ms: 1_000,
        target_lifetime_ms: 900,
        replace_after_loss: true,
    }
}

#[tokio::test]
async fn a_target_is_durable_before_effect_dispatch_and_reuses_without_a_write() {
    let registry = MemoryTargetRegistry::default();
    let first = request(1, "reservation-1", "generation-1");
    let AcquireOutcome::Acquired(lease) = registry.acquire(&first).await.unwrap() else {
        panic!("first call must acquire")
    };
    let target = physical("mvm-1", "guest-generation-1");
    let InstallOutcome::Installed(installed) = registry
        .install(&lease, &target, first.now_ms)
        .await
        .unwrap()
    else {
        panic!("lease must install")
    };
    // The adapter may only dispatch after it possesses this installed proof.
    assert_eq!(installed.target_ref, "mvm-1");
    assert_eq!(installed.generation, "guest-generation-1");

    let retry = request(2, "reservation-2", "generation-2");
    assert!(matches!(
        registry.acquire(&retry).await.unwrap(),
        AcquireOutcome::Installed(InstalledTarget { target_ref, .. }) if target_ref == "mvm-1"
    ));
}

#[tokio::test]
async fn crash_after_run_before_install_retains_capacity_until_the_orphan_lifetime_ends() {
    let registry = MemoryTargetRegistry::with_capacity(1_024);
    let effects = AtomicUsize::new(0);

    let first = request(100, "reservation-old", "generation-old");
    let AcquireOutcome::Acquired(_old_lease) = registry.acquire(&first).await.unwrap() else {
        panic!("first worker acquires")
    };
    let orphan = physical("mvm-idle-orphan", "guest-generation-old");
    // Crash here: the provider launch returned, but no install CAS and therefore no code path has the
    // InstalledTarget proof accepted by the dispatcher.
    assert_eq!(effects.load(Ordering::SeqCst), 0);

    // Once the target's physical hard deadline has passed, retrying Run cannot recover a live
    // target. The target row and counter therefore remain charged through the conservative
    // uncertainty fence instead of reusing the slot for a possible second VM.
    let second = request(1_099, "reservation-new", "generation-new");
    assert!(matches!(
        registry.acquire(&second).await.unwrap(),
        AcquireOutcome::Pending { .. }
    ));
    assert_eq!(registry.reserved_mib(), 1_024);
    assert_eq!(effects.load(Ordering::SeqCst), 0);

    // Only once the configured possible-target lifetime has elapsed may the same charged slot
    // be reclaimed. Production sets that guard to the provider's 8h wall plus skew.
    let third = request(1_101, "reservation-new", "generation-new");
    let AcquireOutcome::Acquired(new_lease) = registry.acquire(&third).await.unwrap() else {
        panic!("guarded lease may be reclaimed only after its possible target lifetime")
    };
    let target = physical("mvm-routable", "guest-generation-new");
    let InstallOutcome::Installed(installed) = registry
        .install(&new_lease, &target, third.now_ms)
        .await
        .unwrap()
    else {
        panic!("replacement installs")
    };
    effects.fetch_add(1, Ordering::SeqCst);
    assert_eq!(installed.target_ref, "mvm-routable");
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(registry.reserved_mib(), 1_024);
    assert_eq!(orphan.target_ref, "mvm-idle-orphan");
}

#[tokio::test]
async fn crash_after_provider_success_replays_one_exact_run_and_installs_one_target() {
    #[derive(Default)]
    struct IdempotentProvider {
        targets: Mutex<BTreeMap<String, (DurableLaunchRequest, PhysicalTarget)>>,
        creations: AtomicUsize,
    }

    impl IdempotentProvider {
        fn run(&self, lease: &MaterializationLease) -> PhysicalTarget {
            let mut targets = self.targets.lock().unwrap();
            if let Some((request, target)) = targets.get(&lease.reservation_id) {
                assert_eq!(
                    request, &lease.launch_request,
                    "same token requires exact params"
                );
                return target.clone();
            }
            let target = PhysicalTarget::new(
                format!("mvm-{}", self.creations.fetch_add(1, Ordering::SeqCst) + 1),
                lease.generation.clone(),
                control_token(),
            )
            .unwrap();
            targets.insert(
                lease.reservation_id.clone(),
                (lease.launch_request.clone(), target.clone()),
            );
            target
        }
    }

    let registry = MemoryTargetRegistry::with_capacity(1_024);
    let provider = IdempotentProvider::default();
    let first = request(1, "reservation-stable", "generation-stable");
    let AcquireOutcome::Acquired(first_lease) = registry.acquire(&first).await.unwrap() else {
        panic!("first worker acquires")
    };
    assert!(!first_lease.recovery_attempt);
    let first_target = provider.run(&first_lease);
    // Crash after the provider accepted the launch and returned the target, before the install CAS.

    let retry = request(102, "reservation-unused", "generation-unused");
    let AcquireOutcome::Acquired(recovery_lease) = registry.acquire(&retry).await.unwrap() else {
        panic!("expired attempt ownership is recoverable")
    };
    assert_eq!(recovery_lease.reservation_id, first_lease.reservation_id);
    assert_eq!(recovery_lease.generation, first_lease.generation);
    assert_eq!(recovery_lease.launch_request, first_lease.launch_request);
    assert_ne!(recovery_lease.attempt_id, first_lease.attempt_id);
    assert!(recovery_lease.recovery_attempt);

    let recovered_target = provider.run(&recovery_lease);
    assert_eq!(recovered_target, first_target);
    assert_eq!(provider.creations.load(Ordering::SeqCst), 1);
    let InstallOutcome::Installed(installed) = registry
        .install(&recovery_lease, &recovered_target, retry.now_ms)
        .await
        .unwrap()
    else {
        panic!("recovered exact target installs")
    };
    assert_eq!(installed.target_ref, first_target.target_ref);
    assert_eq!(registry.reserved_mib(), 1_024);
}

#[tokio::test]
async fn initial_attested_no_target_refunds_capacity() {
    let materializer = TargetMaterializer::new(
        MemoryTargetRegistry::with_capacity(1_024),
        RejectingLauncher(LaunchError::KnownNoTarget(
            "provider rejected before admission".into(),
        )),
    );
    let request = request(1, "reservation-1", "generation-1");
    assert!(matches!(
        materializer.ensure(&request).await,
        Err(MaterializationError::LaunchRejected(_))
    ));
    assert_eq!(materializer.registry().reserved_mib(), 0);
    assert!(
        materializer
            .registry()
            .get(&request.key)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn recovery_provider_errors_never_refund_a_possible_existing_target() {
    let failures = [
        LaunchError::Capacity {
            scope: "provider_account".into(),
            retry_after_ms: 1_000,
            message: "quota".into(),
        },
        LaunchError::KnownNoTarget("idempotent replay returned no target".into()),
        LaunchError::RetryableKnownNoTarget("provider throttled replay".into()),
        LaunchError::OutcomeUnknown("transport closed".into()),
    ];
    for (index, failure) in failures.into_iter().enumerate() {
        let materializer = TargetMaterializer::new(
            MemoryTargetRegistry::with_capacity(1_024),
            RejectingLauncher(failure),
        );
        let first = request(1, "reservation-stable", "generation-stable");
        let AcquireOutcome::Acquired(first_lease) =
            materializer.registry().acquire(&first).await.unwrap()
        else {
            panic!("first worker acquires")
        };
        assert!(!first_lease.recovery_attempt);

        let retry = request(102, "reservation-unused", "generation-unused");
        let error = materializer.ensure(&retry).await.unwrap_err();
        match index {
            0 => assert!(matches!(error, MaterializationError::Capacity { .. })),
            1 | 3 => assert!(matches!(
                error,
                MaterializationError::LaunchOutcomeUnknown(_)
            )),
            2 => assert!(matches!(error, MaterializationError::LaunchRetryable(_))),
            _ => unreachable!(),
        }
        let record = materializer
            .registry()
            .get(&first.key)
            .await
            .unwrap()
            .expect("recovery failure retains the exact target reservation");
        assert!(matches!(
            record.state,
            DurableTargetState::Materializing {
                ref reservation_id,
                ..
            } if reservation_id == &first_lease.reservation_id
        ));
        assert_eq!(materializer.registry().reserved_mib(), 1_024);
    }
}

#[tokio::test]
async fn concurrent_first_call_uses_a_short_poll_without_shortening_the_safety_lease() {
    let registry = MemoryTargetRegistry::with_capacity(1_024);
    let mut first = request(1, "reservation-old", "generation-old");
    first.lease_duration_ms = 8 * 60 * 60 * 1_000 + 5 * 60 * 1_000;
    first.target_lifetime_ms = 8 * 60 * 60 * 1_000;
    let AcquireOutcome::Acquired(lease) = registry.acquire(&first).await.unwrap() else {
        panic!("first worker acquires")
    };

    let mut retry = first.clone();
    retry.now_ms = 2;
    retry.reservation_id = "reservation-retry".into();
    retry.generation = "generation-retry".into();
    let AcquireOutcome::Pending { retry_after_ms, .. } = registry.acquire(&retry).await.unwrap()
    else {
        panic!("concurrent caller waits for the installed proof")
    };
    assert!((1..=MAX_MATERIALIZATION_POLL_MS).contains(&retry_after_ms));
    let record = registry.get(&first.key).await.unwrap().unwrap();
    assert!(matches!(
        record.state,
        DurableTargetState::Materializing {
            lease_expires_at_ms,
            ..
        } if lease_expires_at_ms == lease.lease_expires_at_ms
    ));
    assert_eq!(registry.reserved_mib(), 1_024);
}

#[tokio::test]
async fn stale_worker_cannot_install_or_execute_after_lease_takeover() {
    let registry = MemoryTargetRegistry::default();
    let first = request(1, "reservation-old", "generation-old");
    let AcquireOutcome::Acquired(old) = registry.acquire(&first).await.unwrap() else {
        panic!("first worker acquires")
    };
    let second = request(1_002, "reservation-new", "generation-new");
    let AcquireOutcome::Acquired(new) = registry.acquire(&second).await.unwrap() else {
        panic!("second worker takes expired lease")
    };
    let stale_target = physical("mvm-stale", "guest-generation-stale");
    assert_eq!(
        registry
            .install(&old, &stale_target, second.now_ms)
            .await
            .unwrap(),
        InstallOutcome::ReservationLost
    );
    let current_target = physical("mvm-current", "guest-generation-current");
    assert!(matches!(
        registry
            .install(&new, &current_target, second.now_ms)
            .await
            .unwrap(),
        InstallOutcome::Installed(_)
    ));
}

#[tokio::test]
async fn crash_after_effect_before_brain_receipt_dedupes_on_the_installed_guest() {
    let registry = MemoryTargetRegistry::default();
    let first = request(1, "reservation-1", "generation-1");
    let AcquireOutcome::Acquired(lease) = registry.acquire(&first).await.unwrap() else {
        panic!("first worker acquires")
    };
    let target = physical("mvm-1", "guest-generation-1");
    let InstallOutcome::Installed(installed) = registry
        .install(&lease, &target, first.now_ms)
        .await
        .unwrap()
    else {
        panic!("target installs")
    };

    let mut guest = crate::operation::OperationRegistry::new(8, 4096);
    let first_reservation = guest.reserve("operation-1", &"a".repeat(64), 1024).unwrap();
    assert_eq!(first_reservation, crate::operation::Reservation::New);
    let effects = AtomicUsize::new(0);
    effects.fetch_add(1, Ordering::SeqCst);
    // Brain did not receive the receipt. It retries the durable intent against target_ref.
    let retry_reservation = guest.reserve("operation-1", &"a".repeat(64), 1024).unwrap();
    assert_eq!(retry_reservation, crate::operation::Reservation::Existing);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(installed.target_ref, "mvm-1");
}

#[tokio::test]
async fn conflicting_digest_is_permanent_and_never_reaches_the_effect_body() {
    let registry = MemoryTargetRegistry::default();
    let request = request(1, "reservation-1", "generation-1");
    let AcquireOutcome::Acquired(lease) = registry.acquire(&request).await.unwrap() else {
        panic!("first worker acquires")
    };
    let target = physical("mvm-1", "guest-generation-1");
    let InstallOutcome::Installed(_installed) = registry
        .install(&lease, &target, request.now_ms)
        .await
        .unwrap()
    else {
        panic!("target installs")
    };

    let mut guest = crate::operation::OperationRegistry::new(8, 4096);
    let effects = AtomicUsize::new(0);
    let dispatch = |guest: &mut crate::operation::OperationRegistry,
                    digest: &str|
     -> Result<crate::operation::Reservation, crate::operation::OperationError> {
        let reservation = guest.reserve("operation-1", digest, 1024)?;
        if reservation == crate::operation::Reservation::New {
            effects.fetch_add(1, Ordering::SeqCst);
        }
        Ok(reservation)
    };
    assert_eq!(
        dispatch(&mut guest, &"a".repeat(64)),
        Ok(crate::operation::Reservation::New)
    );
    assert_eq!(
        dispatch(&mut guest, &"b".repeat(64)),
        Err(crate::operation::OperationError::IdempotencyConflict)
    );
    // A permanent conflict is answered at reservation; dispatch never invokes user code.
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn different_target_spec_is_a_permanent_conflict() {
    let registry = MemoryTargetRegistry::default();
    registry
        .acquire(&request(1, "reservation-1", "generation-1"))
        .await
        .unwrap();
    let mut conflict = request(2, "reservation-2", "generation-2");
    conflict.spec = target_spec(ConnectorClass::Public);
    assert_eq!(
        registry.acquire(&conflict).await,
        Err(MaterializationError::SpecConflict)
    );
}

#[tokio::test]
async fn plane_capacity_is_reserved_atomically_and_refunded_once() {
    let registry = MemoryTargetRegistry::with_capacity(2_048);
    let mut first = request(1, "reservation-1", "generation-1");
    first.key = TargetKey::for_default_target("root-1").unwrap();
    let mut second = request(1, "reservation-2", "generation-2");
    second.key = TargetKey::for_default_target("root-2").unwrap();
    let mut third = request(1, "reservation-3", "generation-3");
    third.key = TargetKey::for_default_target("root-3").unwrap();
    let AcquireOutcome::Acquired(first_lease) = registry.acquire(&first).await.unwrap() else {
        panic!("first target reserves")
    };
    assert!(matches!(
        registry.acquire(&second).await.unwrap(),
        AcquireOutcome::Acquired(_)
    ));
    assert_eq!(registry.reserved_mib(), 2_048);
    assert!(matches!(
        registry.acquire(&third).await,
        Err(MaterializationError::Capacity {
            retry_after_ms: 1_000,
            ..
        })
    ));

    let target = physical("mvm-1", "guest-generation-1");
    let InstallOutcome::Installed(installed) =
        registry.install(&first_lease, &target, 2).await.unwrap()
    else {
        panic!("first target installs")
    };
    registry
        .mark_closed(&installed, Disposition::Terminated, "explicit cleanup", 3)
        .await
        .unwrap();
    // Idempotent terminal retry does not decrement twice.
    registry
        .mark_closed(&installed, Disposition::Terminated, "explicit cleanup", 3)
        .await
        .unwrap();
    assert_eq!(registry.reserved_mib(), 1_024);
    assert!(matches!(
        registry.acquire(&third).await.unwrap(),
        AcquireOutcome::Acquired(_)
    ));
}

#[tokio::test]
async fn scheduled_hard_deadline_reconciliation_reclaims_abandoned_capacity() {
    let registry = MemoryTargetRegistry::with_capacity(5 * 1_024);
    let mut installed_targets = Vec::new();
    for index in 0..5 {
        let mut target = request(
            1,
            &format!("reservation-{index}"),
            &format!("generation-{index}"),
        );
        target.key = TargetKey::for_default_target(format!("root-{index}")).unwrap();
        let AcquireOutcome::Acquired(lease) = registry.acquire(&target).await.unwrap() else {
            panic!("abandoned target reserves")
        };
        let physical = physical(format!("mvm-{index}"), format!("guest-generation-{index}"));
        let InstallOutcome::Installed(installed) = registry
            .install(&lease, &physical, target.now_ms)
            .await
            .unwrap()
        else {
            panic!("abandoned target installs")
        };
        assert_eq!(installed.expires_at_ms, 901);
        installed_targets.push(installed);
    }
    assert_eq!(registry.reserved_mib(), 5 * 1_024);

    // No customer request is needed. Brain journals each returned hard deadline and schedules
    // an exact target inspection/termination; each confirmed transition atomically refunds
    // its one capacity reservation while retaining the logical tombstone.
    for installed in &installed_targets {
        registry
            .mark_closed(
                installed,
                Disposition::Terminated,
                "physical target hard deadline reached",
                901,
            )
            .await
            .unwrap();
    }
    assert_eq!(registry.reserved_mib(), 0);
    for installed in installed_targets {
        assert!(matches!(
            registry.get(&installed.key).await.unwrap().unwrap().state,
            DurableTargetState::Closed {
                disposition: Disposition::Terminated,
                ..
            }
        ));
    }
}

#[tokio::test]
async fn brain_minted_create_generation_is_an_exact_fence() {
    let registry = MemoryTargetRegistry::default();
    let mut first = request(1, "reservation-1", "generation-1");
    first.generation_is_fenced = true;
    let AcquireOutcome::Acquired(lease) = registry.acquire(&first).await.unwrap() else {
        panic!("first create reserves")
    };
    let target = physical("mvm-1", "generation-1");
    registry
        .install(&lease, &target, first.now_ms)
        .await
        .unwrap();

    let mut exact = first.clone();
    exact.now_ms = 2;
    exact.reservation_id = "reservation-exact-retry".into();
    assert!(matches!(
        registry.acquire(&exact).await.unwrap(),
        AcquireOutcome::Installed(_)
    ));

    let mut conflict = exact;
    conflict.generation = "generation-2".into();
    assert_eq!(
        registry.acquire(&conflict).await,
        Err(MaterializationError::SpecConflict)
    );
}

#[tokio::test]
async fn confirmed_no_target_releases_the_exact_lease_and_capacity() {
    let registry = MemoryTargetRegistry::with_capacity(1_024);
    let first = request(1, "reservation-1", "generation-1");
    let AcquireOutcome::Acquired(lease) = registry.acquire(&first).await.unwrap() else {
        panic!("target reserves")
    };
    assert_eq!(registry.reserved_mib(), 1_024);
    registry.expire_lease(&lease, 2).await.unwrap();
    registry.expire_lease(&lease, 3).await.unwrap();
    assert_eq!(registry.reserved_mib(), 0);
    assert!(registry.get(&first.key).await.unwrap().is_none());
}

#[tokio::test]
async fn additional_target_never_rematerializes_after_loss() {
    let registry = MemoryTargetRegistry::default();
    let mut first = request(1, "reservation-1", "generation-1");
    first.key = TargetKey::additional("root-1", "sandbox-1").unwrap();
    first.replace_after_loss = false;
    let AcquireOutcome::Acquired(lease) = registry.acquire(&first).await.unwrap() else {
        panic!("first worker acquires")
    };
    let target = physical("mvm-1", "guest-generation-1");
    let InstallOutcome::Installed(installed) = registry
        .install(&lease, &target, first.now_ms)
        .await
        .unwrap()
    else {
        panic!("target installs")
    };
    registry
        .mark_closed(
            &installed,
            Disposition::Lost,
            "provider lifetime expired",
            10,
        )
        .await
        .unwrap();
    let mut retry = first.clone();
    retry.now_ms = 20;
    retry.reservation_id = "reservation-2".into();
    retry.generation = "generation-2".into();
    assert_eq!(
        registry.acquire(&retry).await.unwrap(),
        AcquireOutcome::Gone
    );
}

#[tokio::test]
async fn terminated_additional_id_remains_fenced_until_explicit_root_purge() {
    let registry = MemoryTargetRegistry::default();
    let mut first = request(1, "reservation-1", "generation-1");
    first.key = TargetKey::additional("root-1", "sandbox-1").unwrap();
    first.replace_after_loss = false;
    let AcquireOutcome::Acquired(lease) = registry.acquire(&first).await.unwrap() else {
        panic!("first worker acquires")
    };
    let target = physical("mvm-1", "guest-generation-1");
    let InstallOutcome::Installed(installed) = registry
        .install(&lease, &target, first.now_ms)
        .await
        .unwrap()
    else {
        panic!("target installs")
    };
    registry
        .mark_closed(
            &installed,
            Disposition::Terminated,
            "explicit lifecycle operation",
            10,
        )
        .await
        .unwrap();

    let mut retry = first;
    retry.now_ms = u64::MAX / 2;
    retry.reservation_id = "reservation-after-arbitrary-delay".into();
    retry.generation = "generation-after-arbitrary-delay".into();
    assert_eq!(
        registry.acquire(&retry).await.unwrap(),
        AcquireOutcome::Terminated
    );
}

#[test]
fn identifiers_match_the_brain_contract_boundary() {
    assert!(TargetKey::for_default_target("A._:-9").is_ok());
    for invalid in ["", "-starts-wrong", "has space", "é", &"a".repeat(129)] {
        assert!(
            TargetKey::for_default_target(invalid).is_err(),
            "{invalid:?}"
        );
    }
}

#[test]
fn control_tokens_are_exact_and_never_debug_formatted() {
    let raw = format!("control-{}", "c".repeat(64));
    let token = ControlToken::new(raw.clone()).unwrap();
    assert_eq!(token.expose(), raw);
    assert_eq!(format!("{token:?}"), "ControlToken([redacted])");
    for invalid in [
        String::new(),
        format!("control-{}", "c".repeat(63)),
        format!("control-{}", "C".repeat(64)),
        format!("wrong-{}", "c".repeat(64)),
    ] {
        assert_eq!(
            ControlToken::new(invalid).unwrap_err(),
            SecretError::InvalidControlToken
        );
    }
}
