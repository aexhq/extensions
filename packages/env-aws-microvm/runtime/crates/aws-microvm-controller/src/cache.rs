//! In-memory preparation metadata and content-addressed bundle caches with admission accounting.

use crate::*;

#[derive(Clone)]
pub(crate) struct Preparation {
    pub(crate) request: Arc<PrepareSessionRequest>,
    public_digest: String,
    pub(crate) metadata_bytes: usize,
    pub(crate) last_access: Arc<AtomicU64>,
}

pub(crate) struct CachedBundle {
    pub(crate) bytes: Arc<Vec<u8>>,
    pub(crate) last_access: AtomicU64,
}

#[derive(Debug)]
pub(crate) struct ValidatedPreparedBundle {
    pub(crate) bytes: u64,
    pub(crate) digest: String,
}

/// Session preparation metadata: one LRU bounded by bytes and entries.
pub(crate) struct PreparationStore {
    pub(crate) sessions: HashMap<(String, String), Preparation>,
    pub(crate) root_sessions: HashMap<String, HashSet<(String, String)>>,
    pub(crate) preparation_bytes: usize,
    pub(crate) max_preparation_bytes: usize,
    pub(crate) max_preparations: usize,
    pub(crate) access_clock: AtomicU64,
}

/// Content-addressed bundle bytes: a second, independent LRU. Lookups take `&self` (atomic
/// access clock), so the hot install path can hold the cache read guard.
pub(crate) struct BundleCache {
    pub(crate) bundles: HashMap<String, CachedBundle>,
    pub(crate) bundle_bytes: usize,
    pub(crate) max_bundle_bytes: usize,
    pub(crate) max_bundles: usize,
    pub(crate) access_clock: AtomicU64,
}

/// The two caches share one lock: `install` moves entries into both against a single admission
/// decision, and a purge must not observe one side updated without the other.
pub(crate) struct PreparationCache {
    pub(crate) store: PreparationStore,
    pub(crate) bundles: BundleCache,
}

impl PreparationCache {
    pub(crate) fn with_limit(max_bundle_bytes: usize) -> Self {
        Self::with_limits(
            max_bundle_bytes,
            MAX_CACHED_PREPARATION_BYTES,
            MAX_CACHED_PREPARATIONS,
        )
    }

    pub(crate) fn with_limits(
        max_bundle_bytes: usize,
        max_preparation_bytes: usize,
        max_preparations: usize,
    ) -> Self {
        Self {
            store: PreparationStore {
                sessions: HashMap::new(),
                root_sessions: HashMap::new(),
                preparation_bytes: 0,
                max_preparation_bytes,
                max_preparations,
                access_clock: AtomicU64::new(0),
            },
            bundles: BundleCache {
                bundles: HashMap::new(),
                bundle_bytes: 0,
                max_bundle_bytes,
                max_bundles: MAX_CACHED_BUNDLES,
                access_clock: AtomicU64::new(0),
            },
        }
    }

    pub(crate) fn get(&self, session_id: &str, environment_ref: &str) -> Option<Preparation> {
        self.store.get(session_id, environment_ref)
    }

    pub(crate) fn bundle(&self, digest: &str) -> Option<Arc<Vec<u8>>> {
        self.bundles.get(digest)
    }

    /// Drops at most `limit` logical preparations and their bundle references.
    pub(crate) fn purge_root_page(&mut self, root_id: &str, limit: usize) -> bool {
        self.store.purge_root_page(root_id, limit)
    }
}

impl PreparationStore {
    pub(crate) fn get(&self, session_id: &str, environment_ref: &str) -> Option<Preparation> {
        let access = self
            .access_clock
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        self.sessions
            .get(&(session_id.to_owned(), environment_ref.to_owned()))
            .cloned()
            .inspect(|preparation| {
                preparation.last_access.store(access, Ordering::Relaxed);
            })
    }

    fn remove_session(&mut self, key: &(String, String)) -> Option<Preparation> {
        let removed = self.sessions.remove(key)?;
        self.preparation_bytes = self
            .preparation_bytes
            .saturating_sub(removed.metadata_bytes);
        let root_id = removed.request.root_id.to_string();
        if let Some(sessions) = self.root_sessions.get_mut(&root_id) {
            sessions.remove(key);
            if sessions.is_empty() {
                self.root_sessions.remove(&root_id);
            }
        }
        Some(removed)
    }

    fn evict_preparations_to_fit(
        &mut self,
        additional_bytes: usize,
        additional_entries: usize,
        protected_key: &(String, String),
    ) -> EnvironmentResult<()> {
        loop {
            let bytes_fit = self
                .preparation_bytes
                .checked_add(additional_bytes)
                .is_some_and(|total| total <= self.max_preparation_bytes);
            let entries_fit = self
                .sessions
                .len()
                .checked_add(additional_entries)
                .is_some_and(|total| total <= self.max_preparations);
            if bytes_fit && entries_fit {
                return Ok(());
            }
            let candidate = self
                .sessions
                .iter()
                .filter(|(key, _)| *key != protected_key)
                .min_by_key(|(_, preparation)| preparation.last_access.load(Ordering::Relaxed))
                .map(|(key, _)| key.clone())
                .ok_or_else(|| preparation_cache_capacity_error(self.max_preparation_bytes))?;
            self.remove_session(&candidate)
                .expect("preparation eviction candidate exists");
        }
    }
}

impl BundleCache {
    pub(crate) fn get(&self, digest: &str) -> Option<Arc<Vec<u8>>> {
        let access = self
            .access_clock
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        self.bundles.get(digest).map(|bundle| {
            bundle.last_access.store(access, Ordering::Relaxed);
            bundle.bytes.clone()
        })
    }

    /// Makes room without invalidating an in-progress installation. A cached Arc is borrowed
    /// while it is being installed into a guest; only entries owned solely by this cache are
    /// eviction candidates. Immutable preparation metadata intentionally does not pin bytes.
    pub(crate) fn evict_idle_to_fit(
        &mut self,
        additional_bytes: usize,
        additional_entries: usize,
        protected: &HashSet<String>,
    ) -> EnvironmentResult<()> {
        loop {
            let bytes_fit = self
                .bundle_bytes
                .checked_add(additional_bytes)
                .is_some_and(|total| total <= self.max_bundle_bytes);
            let entries_fit = self
                .bundles
                .len()
                .checked_add(additional_entries)
                .is_some_and(|total| total <= self.max_bundles);
            if bytes_fit && entries_fit {
                return Ok(());
            }
            let candidate = self
                .bundles
                .iter()
                .filter(|(digest, bundle)| {
                    !protected.contains(digest.as_str()) && Arc::strong_count(&bundle.bytes) == 1
                })
                .min_by_key(|(_, bundle)| bundle.last_access.load(Ordering::Relaxed))
                .map(|(digest, _)| digest.clone())
                .ok_or_else(|| {
                    if !entries_fit && bytes_fit {
                        bundle_cache_entry_capacity_error()
                    } else {
                        bundle_cache_capacity_error(self.max_bundle_bytes)
                    }
                })?;
            let evicted = self.bundles.remove(&candidate).expect("candidate exists");
            self.bundle_bytes = self.bundle_bytes.saturating_sub(evicted.bytes.len());
        }
    }

    fn insert(&mut self, digest: String, bytes: Arc<Vec<u8>>) {
        self.bundle_bytes += bytes.len();
        let access = self
            .access_clock
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        self.bundles.insert(
            digest,
            CachedBundle {
                bytes,
                last_access: AtomicU64::new(access),
            },
        );
    }
}

impl PreparationCache {
    pub(crate) fn install(
        &mut self,
        request: PrepareSessionRequest,
        environment_ref: String,
        public_digest: String,
        fetched: HashMap<String, Arc<Vec<u8>>>,
    ) -> EnvironmentResult<()> {
        let session_id = request.session_id.to_string();
        let root_id = request.root_id.to_string();
        let key = (session_id, environment_ref);
        if self
            .store
            .sessions
            .get(&key)
            .is_some_and(|old| old.request.root_id != request.root_id)
        {
            return Err(binding_error(
                "prepared session cannot move to a different root",
            ));
        }
        if self
            .store
            .sessions
            .get(&key)
            .is_some_and(|old| old.public_digest != public_digest)
        {
            return Err(binding_error(
                "prepared session immutable routing or bundle seal changed",
            ));
        }
        let required = required_bundle_digests(&request)?;
        if fetched.keys().any(|digest| !required.contains(digest)) {
            return Err(invalid(
                "preparation contains a fetch for an unreferenced bundle",
            ));
        }
        for digest in &required {
            if !fetched.contains_key(digest) && !self.bundles.bundles.contains_key(digest) {
                return Err(error(
                    EnvironmentErrorCode::CapabilityUnavailable,
                    false,
                    "bundle cache recovery requires a fresh preparation fetch",
                ));
            }
        }

        // Preparation metadata is cold-path state and may be reconstructed by Brain. Bound it
        // separately from resident bundle bytes so a large population of dormant sessions cannot
        // grow the shared hosted process without limit. Eviction is safe: the next operation gets
        // CapabilityUnavailable before materialization/effect and Brain supplies a fresh prepare.
        let metadata_bytes = serde_jcs::to_vec(&request)
            .map_err(|_| invalid("preparation metadata cannot be bounded"))?
            .len();
        if metadata_bytes > self.store.max_preparation_bytes || self.store.max_preparations == 0 {
            return Err(preparation_cache_capacity_error(
                self.store.max_preparation_bytes,
            ));
        }
        let prior_metadata_bytes = self
            .store
            .sessions
            .get(&key)
            .map_or(0, |preparation| preparation.metadata_bytes);
        let additional_bytes = metadata_bytes.saturating_sub(prior_metadata_bytes);
        let additional_entries = usize::from(!self.store.sessions.contains_key(&key));
        self.store
            .evict_preparations_to_fit(additional_bytes, additional_entries, &key)?;

        let missing = required
            .iter()
            .filter(|digest| !self.bundles.bundles.contains_key(digest.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let additional_bytes = missing.iter().try_fold(0usize, |total, digest| {
            total
                .checked_add(fetched.get(digest).expect("required fetch checked").len())
                .ok_or_else(|| bundle_cache_capacity_error(self.bundles.max_bundle_bytes))
        })?;
        self.bundles
            .evict_idle_to_fit(additional_bytes, missing.len(), &required)?;
        for digest in &required {
            if !self.bundles.bundles.contains_key(digest) {
                let bytes = fetched.get(digest).expect("required fetch checked").clone();
                self.bundles.insert(digest.clone(), bytes);
            }
            let _ = self.bundles.get(digest);
        }
        if self.store.sessions.contains_key(&key) {
            self.store.remove_session(&key);
        }
        let last_access = self
            .store
            .access_clock
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        self.store.preparation_bytes = self
            .store
            .preparation_bytes
            .checked_add(metadata_bytes)
            .ok_or_else(|| preparation_cache_capacity_error(self.store.max_preparation_bytes))?;
        self.store.sessions.insert(
            key.clone(),
            Preparation {
                request: Arc::new(request),
                public_digest,
                metadata_bytes,
                last_access: Arc::new(AtomicU64::new(last_access)),
            },
        );
        self.store
            .root_sessions
            .entry(root_id)
            .or_default()
            .insert(key);
        debug_assert_eq!(
            self.bundles.bundle_bytes,
            self.bundles
                .bundles
                .values()
                .map(|bundle| bundle.bytes.len())
                .sum::<usize>()
        );
        Ok(())
    }
}

impl PreparationStore {
    /// Drops at most `limit` logical preparations and their bundle references.
    pub(crate) fn purge_root_page(&mut self, root_id: &str, limit: usize) -> bool {
        let keys = self
            .root_sessions
            .get(root_id)
            .into_iter()
            .flat_map(|sessions| sessions.iter().take(limit))
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            let _ = self.remove_session(&key);
        }
        let complete = self
            .root_sessions
            .get(root_id)
            .is_none_or(HashSet::is_empty);
        if complete {
            self.root_sessions.remove(root_id);
        }
        complete
    }
}

impl Default for PreparationCache {
    fn default() -> Self {
        Self::with_limit(DEFAULT_BUNDLE_CACHE_MAX_MIB as usize * MIB)
    }
}

/// Bundle fetch URLs and headers are short-lived bearer authorities. They are consumed while the
/// preparation request is active and must not become part of the process-lifetime session cache.
/// The immutable descriptors and binding-to-digest projection remain in the request, so bundle
/// cache recovery still fails closed and asks Brain for a fresh preparation authority.
pub(crate) fn cacheable_preparation(mut request: PrepareSessionRequest) -> PrepareSessionRequest {
    request.bundles.clear();
    request
}

pub(crate) fn preparation_public_projection(
    request: &PrepareSessionRequest,
) -> EnvironmentResult<serde_json::Value> {
    let mut secret_env_names = request
        .secret_capability
        .iter()
        .flat_map(|capability| capability.env_names.iter())
        .map(|name| name.as_str().to_owned())
        .collect::<Vec<_>>();
    secret_env_names.sort_unstable();
    if secret_env_names.len() > brain_protocol::MAX_SESSION_SECRET_NAMES
        || secret_env_names
            .iter()
            .any(|name| !environment_name_is_valid(name) || reserved_tool_environment(name))
        || secret_env_names.windows(2).any(|pair| pair[0] == pair[1])
    {
        return Err(invalid(
            "secret capability has invalid, reserved, or repeated environment names",
        ));
    }
    Ok(serde_json::json!({
        "bindings": request.bindings,
        "network": request.network,
        "resources": request.resources,
        "root_id": request.root_id,
        // The one-purpose bearer and expiry may be refreshed after a control-process loss, but
        // the declared session environment-name union is part of the immutable preparation.
        "secret_env_names": secret_env_names,
        "session_id": request.session_id,
    }))
}

#[derive(Clone, Copy)]
pub(crate) enum MaterializationMode<'a> {
    LazyEnvironment(TargetLifetime),
    ExplicitEnvironment(&'a str),
    Additional(&'a str),
}

impl<'a> MaterializationMode<'a> {
    pub(crate) fn generation_intent(self) -> Option<&'a str> {
        match self {
            Self::LazyEnvironment(_) => None,
            Self::ExplicitEnvironment(generation) | Self::Additional(generation) => {
                Some(generation)
            }
        }
    }

    pub(crate) fn replace_after_loss(self) -> bool {
        matches!(
            self,
            Self::LazyEnvironment(_) | Self::ExplicitEnvironment(_)
        )
    }

    pub(crate) fn lifetime(self) -> TargetLifetime {
        match self {
            Self::LazyEnvironment(lifetime) => lifetime,
            Self::ExplicitEnvironment(_) | Self::Additional(_) => TargetLifetime::default(),
        }
    }
}

pub(crate) fn zeroize_secret_values(values: &mut HashMap<String, String>) {
    for value in values.values_mut() {
        value.zeroize();
    }
    values.clear();
}

/// One shard derivation for every lock array: SHA-256 over NUL-joined parts, first 8 bytes.
pub(crate) fn shard_index(parts: &[&str], shards: usize) -> usize {
    let mut digest = Sha256::new();
    for (position, part) in parts.iter().enumerate() {
        if position > 0 {
            digest.update([0]);
        }
        digest.update(part.as_bytes());
    }
    let digest = digest.finalize();
    let prefix = u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 prefix"));
    prefix as usize % shards
}

pub(crate) fn secret_install_lock_index(target_ref: &str, session_id: &str) -> usize {
    shard_index(&[target_ref, session_id], SECRET_INSTALL_LOCK_SHARDS)
}

/// Supervisor-owned temporary object. It has no stable external name, is mode 0600, and is
/// removed automatically. Transfer authorities and values are deliberately not retained here.
pub(crate) struct StagedObject {
    pub(crate) file: tempfile::NamedTempFile,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

/// Process-wide bytes admitted for verified bundles that are currently being fetched but are not
/// yet represented in `PreparationCache::bundle_bytes`. The reservation uses a synchronous lock
/// only for integer accounting, so its `Drop` path remains cancellation-safe across network
/// awaits.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BundleFetchInFlight {
    pub(crate) bytes: usize,
    pub(crate) entries: usize,
}

#[derive(Debug)]
pub(crate) struct BundleFetchReservation {
    pub(crate) reserved: Arc<StdMutex<BundleFetchInFlight>>,
    pub(crate) bytes: usize,
    pub(crate) entries: usize,
}

impl BundleFetchReservation {
    /// Reserves the declared upper bound, rather than the eventual response size. This keeps the
    /// cache plus every concurrent cold fetch below one process-wide limit even if all servers
    /// return their maximum response at once.
    pub(crate) fn admit(
        reserved: Arc<StdMutex<BundleFetchInFlight>>,
        cached_bytes: usize,
        cached_entries: usize,
        fetch_bytes: usize,
        fetch_entries: usize,
        cache_limit_bytes: usize,
        fetch_limit_bytes: usize,
    ) -> EnvironmentResult<Self> {
        let mut in_flight = reserved
            .lock()
            .map_err(|error| temporary_from("bundle fetch admission is unavailable", error))?;
        let projected_fetch = in_flight
            .bytes
            .checked_add(fetch_bytes)
            .ok_or_else(|| bundle_fetch_capacity_error(fetch_limit_bytes))?;
        if projected_fetch > fetch_limit_bytes {
            return Err(bundle_fetch_capacity_error(fetch_limit_bytes));
        }
        let admitted = cached_bytes
            .checked_add(in_flight.bytes)
            .and_then(|bytes| bytes.checked_add(fetch_bytes))
            .ok_or_else(|| bundle_cache_capacity_error(cache_limit_bytes))?;
        if admitted > cache_limit_bytes {
            return Err(bundle_cache_capacity_error(cache_limit_bytes));
        }
        let admitted_entries = cached_entries
            .checked_add(in_flight.entries)
            .and_then(|entries| entries.checked_add(fetch_entries))
            .ok_or_else(|| bundle_cache_capacity_error(cache_limit_bytes))?;
        if admitted_entries > MAX_CACHED_BUNDLES {
            return Err(bundle_cache_entry_capacity_error());
        }
        in_flight.bytes = projected_fetch;
        in_flight.entries += fetch_entries;
        drop(in_flight);
        Ok(Self {
            reserved,
            bytes: fetch_bytes,
            entries: fetch_entries,
        })
    }
}

impl Drop for BundleFetchReservation {
    fn drop(&mut self) {
        let mut reserved = self
            .reserved
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        reserved.bytes = reserved.bytes.saturating_sub(self.bytes);
        reserved.entries = reserved.entries.saturating_sub(self.entries);
    }
}
