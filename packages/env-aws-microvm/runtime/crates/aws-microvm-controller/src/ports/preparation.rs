//! `SessionPreparationPort`: prepare, materialize, dematerialize, purge.

use crate::*;

#[async_trait]
impl SessionPreparationPort for AwsMicrovmEnvironment {
    async fn prepare(&self, request: PrepareSessionRequest) -> EnvironmentResult<PreparedSession> {
        let (required_bundles, supplied_bundles, projection, environment_ref) =
            self.validate_prepare_request(&request).await?;
        let digest = self
            .persist_preparation_definitions(&request, &environment_ref, &projection)
            .await?;
        let preparation_ref = format!("preparation:{digest}");
        let fetched = self
            .admit_and_fetch_bundles(&required_bundles, &supplied_bundles)
            .await?;
        let request = cacheable_preparation(request);
        self.preparation_cache.write().await.install(
            request,
            environment_ref,
            digest,
            fetched.bundles,
        )?;
        // `fetched` still holds the fetch reservation and resident-bundle borrows; they are
        // released only now that the bytes are accounted inside the cache.
        drop(fetched.guards);
        Ok(PreparedSession {
            preparation_ref: preparation_ref.parse().expect("preparation ref"),
        })
    }

    async fn materialize(&self, request: CreateSandboxRequest) -> EnvironmentResult<SandboxStatus> {
        if request.target.kind != TargetKind::Environment || request.target.sandbox_id.is_some() {
            return Err(invalid("environment target is required"));
        }
        let preparation = self
            .preparation_for_root(
                request.target.session_id.as_str(),
                request.target.root_id.as_str(),
                request.target.binding_ref.as_str(),
            )
            .await?;
        require_exact_root_seal(&request, &preparation.request)?;
        let installed = self
            .materialize(
                target_key(&request.target)?,
                request.target.session_id.as_str(),
                &preparation.request.resources,
                &preparation.request.network,
                request.resource_class.as_str(),
                MaterializationMode::ExplicitEnvironment(request.generation_intent.as_str()),
            )
            .await?;
        Ok(running_status(request.target, &installed))
    }

    async fn dematerialize(&self, target: SandboxTarget) -> EnvironmentResult<SandboxStatus> {
        if target.kind != TargetKind::Environment || target.sandbox_id.is_some() {
            return Err(invalid("environment target is required"));
        }
        // An abandoned materialization (its owning turn cancelled between the capacity
        // reservation and completion; attempt window lapsed with no live owner) never
        // completes on its own — and session end must be able to resolve it rather than
        // retry against "in progress" forever: the reservation pins plane capacity, and
        // deletion gates on end, so nothing else can ever run the recovery. Drive the
        // same exact recovery the root purge uses, then let the caller's retry observe
        // the terminal state (gone / not materialized reads as ended).
        let key = target_key(&target)?;
        if let Some(record) = self
            .plane
            .registry
            .get(&key)
            .await
            .map_err(materialization_error)?
            && let environment_core::materialization::DurableTargetState::Materializing {
                attempt_expires_at_ms,
                ..
            } = &record.state
            && *attempt_expires_at_ms <= now_ms()
        {
            let _ = self.recover_materializing(&record).await?;
            return Err(temporary(
                "abandoned sandbox materialization recovery progressed; retry",
            ));
        }
        let installed = self.resolve_target(&target, None).await?;
        self.terminate_target(&installed, "explicit environment lifecycle operation")
            .await?;
        Ok(terminated_status(
            target,
            &installed,
            "explicit environment lifecycle operation",
        ))
    }

    async fn purge_tree(&self, root_id: &str) -> EnvironmentResult<()> {
        const MAX_TARGETS_PER_PURGE_ATTEMPT: usize = 25;
        let page = self
            .plane
            .registry
            .list_root(root_id, None, MAX_TARGETS_PER_PURGE_ATTEMPT)
            .await
            .map_err(materialization_error)?;
        let mut unresolved_materialization = false;
        for record in page.items {
            if matches!(self.purge_one_target(&record).await?, PurgeStep::Unresolved) {
                unresolved_materialization = true;
            }
        }
        let targets_remain = !self
            .plane
            .registry
            .list_root(root_id, None, 1)
            .await
            .map_err(materialization_error)?
            .items
            .is_empty();
        if unresolved_materialization || targets_remain {
            return Err(temporary(
                "sandbox tree cleanup is incomplete; bounded purge will retry",
            ));
        }
        let definitions_purged = self
            .plane
            .definitions
            .purge_root_page(root_id, MAX_TARGETS_PER_PURGE_ATTEMPT)
            .await
            .map_err(definition_error)?;
        if !definitions_purged {
            return Err(temporary(
                "sandbox definition cleanup is incomplete; bounded purge will retry",
            ));
        }
        if !self
            .preparation_cache
            .write()
            .await
            .purge_root_page(root_id, MAX_TARGETS_PER_PURGE_ATTEMPT)
        {
            return Err(temporary(
                "session preparation cleanup is incomplete; bounded purge will retry",
            ));
        }
        Ok(())
    }
}

/// Fetched bundle bytes plus the accounting guards that must outlive cache installation: the
/// in-flight fetch reservation and the borrows pinning resident bundles against eviction.
struct FetchedBundles {
    bundles: HashMap<String, Arc<Vec<u8>>>,
    guards: (BundleFetchReservation, Vec<Arc<Vec<u8>>>),
}

impl AwsMicrovmEnvironment {
    /// Validates bounds, the public projection, the physical root seal, and the immutable
    /// binding-to-bundle projection — before reading definitions, writing durable state, or
    /// consuming any one-purpose authority.
    async fn validate_prepare_request(
        &self,
        request: &PrepareSessionRequest,
    ) -> EnvironmentResult<(
        HashMap<String, ValidatedPreparedBundle>,
        HashMap<String, BundleFetch>,
        serde_json::Value,
        String,
    )> {
        if request.bundles.len() > MAX_PREPARED_BUNDLES
            || request.bindings.len() > MAX_PREPARED_BUNDLES
        {
            return Err(invalid("preparation exceeds the bundle/binding bound"));
        }
        let projection = preparation_public_projection(request)?;
        target_spec(
            &self.plane.cfg,
            &request.resources,
            &request.network,
            RESOURCE_CLASS,
        )?;
        let (required_bundles, environment_ref) = self.validate_prepared_bindings(request).await?;
        let mut supplied_bundles = HashMap::with_capacity(request.bundles.len());
        for fetch in &request.bundles {
            let digest = fetch.bundle_digest.to_string();
            if !required_bundles.contains_key(&digest) {
                return Err(invalid(
                    "preparation contains a fetch for an unreferenced bundle",
                ));
            }
            if supplied_bundles.insert(digest, fetch.clone()).is_some() {
                return Err(invalid("preparation repeats a bundle fetch"));
            }
        }
        Ok((
            required_bundles,
            supplied_bundles,
            projection,
            environment_ref,
        ))
    }

    /// Writes the immutable root seal and preparation definition rows; returns the canonical
    /// preparation digest.
    async fn persist_preparation_definitions(
        &self,
        request: &PrepareSessionRequest,
        environment_ref: &str,
        projection: &serde_json::Value,
    ) -> EnvironmentResult<String> {
        let digest = canonical_digest(projection)
            .map_err(|_| invalid("preparation cannot be canonicalized"))?;
        let root_seal = serde_json::json!({
            "network": request.network,
            "resource_class": RESOURCE_CLASS,
            "resources": request.resources,
            "root_id": request.root_id,
        });
        let root_record = DefinitionRecord::canonical(
            request.root_id.as_str(),
            DefinitionKind::RootSeal,
            "physical",
            &root_seal,
            now_ms(),
        )
        .map_err(definition_error)?;
        self.plane
            .definitions
            .install(&root_record)
            .await
            .map_err(root_seal_error)?;
        let record = DefinitionRecord::canonical(
            request.root_id.as_str(),
            DefinitionKind::Preparation,
            format!(
                "prep_{}",
                &hex::encode(Sha256::digest(
                    format!("{}\0{environment_ref}", request.session_id.as_str()).as_bytes()
                ))[..24]
            ),
            projection,
            now_ms(),
        )
        .map_err(definition_error)?;
        self.plane
            .definitions
            .install(&record)
            .await
            .map_err(definition_error)?;
        Ok(digest.to_string())
    }

    /// Admits missing bundles against the process-wide fetch bound and fetches them. A replay
    /// for a still-cached bundle is network-free. Admission runs while holding the cache guard
    /// so cache bytes and concurrent reservations form one atomic bound; the guard is released
    /// before any network await.
    async fn admit_and_fetch_bundles(
        &self,
        required_bundles: &HashMap<String, ValidatedPreparedBundle>,
        supplied_bundles: &HashMap<String, BundleFetch>,
    ) -> EnvironmentResult<FetchedBundles> {
        let (missing_fetches, fetch_reservation, resident_borrows) = {
            let mut cache = self.preparation_cache.write().await;
            let mut missing_fetches = Vec::new();
            let mut resident_borrows = Vec::new();
            let mut fetch_bytes = 0usize;
            for (digest, seal) in required_bundles {
                if let Some(bytes) = cache.bundle(digest) {
                    // Keep this Arc borrowed until fetched bundles are installed. Another
                    // preparation may evict unrelated idle entries while network I/O is pending,
                    // but it cannot turn this exact preparation into a post-fetch cache miss.
                    resident_borrows.push(bytes);
                    continue;
                }
                let fetch = supplied_bundles.get(digest).ok_or_else(|| {
                    error(
                        EnvironmentErrorCode::CapabilityUnavailable,
                        false,
                        "bundle cache recovery requires a fresh preparation fetch",
                    )
                })?;
                if fetch.expires_at_ms.get() <= now_ms()
                    || fetch.max_bytes.get() as usize > environment_wire::MAX_TOOL_BUNDLE_BYTES
                    || fetch.max_bytes.get() < seal.bytes
                {
                    return Err(invalid(
                        "bundle fetch authority is expired or exceeds the bundle bound",
                    ));
                }
                fetch_bytes = fetch_bytes
                    .checked_add(fetch.max_bytes.get() as usize)
                    .ok_or_else(|| bundle_fetch_capacity_error(self.bundle_fetch_max_bytes))?;
                missing_fetches.push(fetch.clone());
            }
            let in_flight = *self.bundle_fetch_reserved.lock().map_err(|error| {
                temporary_from("bundle fetch admission lock is unavailable", error)
            })?;
            let cache_limit = cache.bundles.max_bundle_bytes;
            cache.bundles.evict_idle_to_fit(
                in_flight
                    .bytes
                    .checked_add(fetch_bytes)
                    .ok_or_else(|| bundle_fetch_capacity_error(self.bundle_fetch_max_bytes))?,
                in_flight
                    .entries
                    .checked_add(missing_fetches.len())
                    .ok_or_else(|| bundle_cache_capacity_error(cache_limit))?,
                &required_bundles.keys().cloned().collect(),
            )?;
            let reservation = BundleFetchReservation::admit(
                self.bundle_fetch_reserved.clone(),
                cache.bundles.bundle_bytes,
                cache.bundles.bundles.len(),
                fetch_bytes,
                missing_fetches.len(),
                cache.bundles.max_bundle_bytes,
                self.bundle_fetch_max_bytes,
            )?;
            (missing_fetches, reservation, resident_borrows)
        };
        let fetched_results =
            futures_util::stream::iter(missing_fetches.into_iter().map(|fetch| {
                let expected_bytes = required_bundles
                    .get(fetch.bundle_digest.as_str())
                    .expect("required fetch was validated")
                    .bytes;
                async move {
                    let digest = fetch.bundle_digest.to_string();
                    let bytes = fetch_bundle(self.plane.guest.http(), &fetch).await?;
                    if bytes.len() as u64 != expected_bytes {
                        return Err(invalid(
                            "fetched bundle bytes conflict with the immutable descriptor",
                        ));
                    }
                    EnvironmentResult::Ok((digest, Arc::new(bytes)))
                }
            }))
            .buffer_unordered(8)
            .collect::<Vec<_>>()
            .await;
        let mut bundles = HashMap::with_capacity(fetched_results.len());
        for result in fetched_results {
            let (digest, bytes) = result?;
            let prior = bundles.insert(digest, bytes);
            debug_assert!(prior.is_none());
        }
        Ok(FetchedBundles {
            bundles,
            guards: (fetch_reservation, resident_borrows),
        })
    }
}

/// One purge attempt's verdict for one durable target record.
enum PurgeStep {
    /// The record is fully projected to terminal and its row removed.
    Purged,
    /// The record needs a later bounded retry (lease race, live attempt, or CAS loss).
    Unresolved,
}

impl AwsMicrovmEnvironment {
    async fn purge_one_target(
        &self,
        record: &environment_core::materialization::DurableTargetRecord,
    ) -> EnvironmentResult<PurgeStep> {
        if let Some(installed) = record.installed() {
            self.terminate_target(&installed, "root purge").await?;
            self.plane
                .registry
                .purge_terminal(&record.key, &record.generation)
                .await
                .map_err(materialization_error)?;
            return Ok(PurgeStep::Purged);
        }
        match &record.state {
            DurableTargetState::Closed { .. } => {
                self.plane
                    .registry
                    .purge_terminal(&record.key, &record.generation)
                    .await
                    .map_err(materialization_error)?;
                Ok(PurgeStep::Purged)
            }
            DurableTargetState::Materializing { .. } => self.recover_materializing(record).await,
            DurableTargetState::Installed { .. } => unreachable!("handled above"),
        }
    }

    /// Resolves a materializing row during root purge: expire a dead lease, replay the exact
    /// provider request when recovery is safe, and terminate whatever target that resolves to.
    async fn recover_materializing(
        &self,
        record: &environment_core::materialization::DurableTargetRecord,
    ) -> EnvironmentResult<PurgeStep> {
        let now = now_ms();
        let lease = record.recovery_lease().map_err(materialization_error)?;
        if lease.lease_expires_at_ms <= now {
            // The lease includes the provider's full possible VM lifetime plus skew.
            // Exact delete/refund is authoritative now; one retry closes an install-CAS
            // race before definition rows are removed.
            self.plane
                .registry
                .expire_lease(&lease, now)
                .await
                .map_err(materialization_error)?;
            // A concurrent install can win the conditional delete. Re-read on the
            // bounded delete retry before purging session definitions.
            return Ok(PurgeStep::Unresolved);
        }
        if lease.target_expires_at_ms <= now || lease.attempt_expires_at_ms > now {
            // Do not replay after the target's provider lifetime, and do not race the
            // worker that currently owns the short attempt. The long fence remains
            // charged until exact recovery or the provider lifetime plus skew ends.
            return Ok(PurgeStep::Unresolved);
        }

        let recovery = recovery_request(&lease, now);
        let recovered_lease = match self
            .plane
            .registry
            .acquire(&recovery)
            .await
            .map_err(materialization_error)?
        {
            environment_core::materialization::AcquireOutcome::Acquired(recovered)
                if recovered.recovery_attempt =>
            {
                recovered
            }
            environment_core::materialization::AcquireOutcome::Acquired(fresh) => {
                // The row disappeared between the list and acquisition. `acquire`
                // may have installed a fresh reservation, but no provider call has
                // happened. Remove it immediately; deletion must never materialize a
                // new target merely to discover that the old row was already gone.
                self.plane
                    .registry
                    .expire_lease(&fresh, now)
                    .await
                    .map_err(materialization_error)?;
                return Ok(PurgeStep::Unresolved);
            }
            environment_core::materialization::AcquireOutcome::Installed(installed) => {
                self.terminate_target(&installed, "root purge").await?;
                self.plane
                    .registry
                    .purge_terminal(&installed.key, &installed.generation)
                    .await
                    .map_err(materialization_error)?;
                return Ok(PurgeStep::Purged);
            }
            environment_core::materialization::AcquireOutcome::Pending { .. }
            | environment_core::materialization::AcquireOutcome::Gone
            | environment_core::materialization::AcquireOutcome::Terminated => {
                return Ok(PurgeStep::Unresolved);
            }
        };

        let launcher = GenerationLauncher::from_durable(self.plane.clone(), &recovered_lease)
            .map_err(materialization_error)?;
        let physical = launcher
            .launch(&recovered_lease)
            .await
            .map_err(recovery_launch_error)
            .map_err(materialization_error)?;
        let installed = match self
            .plane
            .registry
            .install(&recovered_lease, &physical, now_ms())
            .await
            .map_err(materialization_error)?
        {
            environment_core::materialization::InstallOutcome::Installed(installed) => installed,
            environment_core::materialization::InstallOutcome::ReservationLost => {
                // Root deletion owns cleanup. If another transition won the install
                // CAS, destroy the exact recovered physical target and retry the
                // durable projection rather than leaking it.
                launcher
                    .terminate_stale(&physical)
                    .await
                    .map_err(temporary)?;
                return Ok(PurgeStep::Unresolved);
            }
        };
        self.terminate_target(&installed, "root purge").await?;
        self.plane
            .registry
            .purge_terminal(&installed.key, &installed.generation)
            .await
            .map_err(materialization_error)?;
        Ok(PurgeStep::Purged)
    }
}
