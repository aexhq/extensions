//! Production Aex-managed Environment implementation for AWS Lambda MicroVMs.
//!
//! Brain owns the public contract and commits operation intent before dispatch. This adapter owns
//! physical target routing only: a first target reservation and the plane memory counter are one
//! DynamoDB transaction, RunMicrovm remains effect-free, the target is durably installed before
//! any guest request, and established submit calls use Brain's projected `target_ref` without a
//! registry read or write. Observe/cancel/ack carry the exact rooted target and intentionally
//! reconcile that target row so a lost supervisor can be terminated and its capacity refunded.

pub(crate) mod client;
pub(crate) mod definitions;
mod dynamo;
pub(crate) mod registry;

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use brain::environment::{
    EnvironmentPort, EnvironmentResult, SandboxControlPort, SandboxFileContent, SandboxFileList,
    SandboxFileListRequest, SandboxFilesPort, SandboxSearchRequest, SecretDeliveryPort,
    SessionPreparationPort,
};
use brain_protocol::contract::{
    ENVIRONMENT_CONTRACT_DIGEST, canonical_digest, operation_request_digest, sandbox_copy_request_digest,
    sandbox_execution_request_digest, sandbox_file_write_request_digest,
    write_stdin_request_digest,
};
use brain_protocol::environment::{
    AcknowledgeTerminalRequest, Acknowledgement, ArtifactTarget, BundleDescriptor, BundleFetch,
    CancelRequest, CancellationReceipt, CreateSandboxRequest, FileEntry,
    EnvironmentCapability, EnvironmentError, EnvironmentErrorCode, NetworkCeiling, NetworkCeilingDestinationsItem,
    ObjectReference, ObjectTransferAuthority, ObjectTransferAuthorityMethod, ObserveRequest,
    OperationObservation, OperationRef, PrepareSessionRequest, PreparedBindingBundles,
    PreparedSession, RecoveryClass, ResolvedBinding, ResolvedBindingLimits, ResourceCeiling,
    SandboxCopyRequest, SandboxCopyRequestDirection, SandboxCopyResult, SandboxExecutionRequest,
    SandboxFileRequest, SandboxFileWriteRequest, SandboxFileWriteResult, SandboxFileWriteSource,
    SandboxState, SandboxStatus, SandboxTarget, SealedBinding, SecretDeliveryRequest,
    SubmitReceipt, SubmitRequest, TargetKind, WriteStdinReceipt, WriteStdinRequest,
};
use brain_protocol::network::network_ceiling_is_subset;
use futures_util::StreamExt as _;
use environment_core::connector::{ConnectorCatalog, ConnectorClass, GatewayAuthority};
use environment_core::materialization::{
    AcquireTarget, ControlToken, Disposition, DurableLaunchRequest, DurableTargetState,
    InstalledTarget, LaunchError, MaterializationError, MaterializationLease, PhysicalTarget,
    PhysicalTargetLauncher, TargetDirectory, TargetKey, TargetMaterializer, TargetReservations,
    TargetSpec,
};
use environment_egress_gateway::{
    Capability, CapabilityDestination, DestinationProtocol, encode_signed_token,
    unsigned_capability_bytes,
};
use environment_lambda::control::{
    Control, ControlError, ControlPacingConfig, ExactRunMicrovmRequest, is_terminated,
};
use environment_lambda::launch::{self, LaunchFailure};
use environment_policy::MAX_OBJECT_BYTES;
use environment_policy::guest_env::{
    environment_name_is_valid, reserved_tool_environment, secret_material_fits,
};
use environment_wire::{
    AllowlistProxy, FileEffectIdentity, FileEffectKind, FileEffectReservation,
    FileEffectStoredResult, GuestFileWriteRequest, GuestFileWriteSource, InstallBindingRequest,
    InstallBundleMetadata, InstallObjectMetadata, InstallSecretsRequest, RequestCall,
    ResponseReply, RunPayload,
};
use ipnet::Ipv4Net;
use sha2::{Digest as _, Sha256};
use tokio::sync::{Mutex, RwLock, Semaphore};
use zeroize::Zeroize as _;

use crate::client::{GuestClient, control_error, error, temporary};
use crate::definitions::{
    DefinitionError, DefinitionKind, DefinitionRecord, DynamoDefinitionRegistry,
};
use crate::registry::DynamoTargetRegistry;

const ENVIRONMENT_ID: &str = "aex-aws-environment";
const RESOURCE_CLASS: &str = "microvm-1gb";
const MIB: usize = 1024 * 1024;
const TARGET_MEMORY_MIB: u64 = environment_lambda::image::MVP_TARGET_MEMORY_MIB as u64;
const MAX_PREPARED_BUNDLES: usize = brain_protocol::MAX_MODEL_TOOLS;
const MAX_CACHED_BUNDLES: usize = 4_096;
const MAX_CACHED_PREPARATIONS: usize = 16_384;
const MAX_CACHED_PREPARATION_BYTES: usize = 64 * MIB;
const MAX_CONCURRENT_BUNDLE_INSTALLS: usize = 4;
const MAX_CONCURRENT_SECRET_INSTALLS: usize = 4;
const SECRET_INSTALL_LOCK_SHARDS: usize = 64;
const FILE_EFFECT_LOCK_SHARDS: usize = 64;
const DEFAULT_BUNDLE_CACHE_MAX_MIB: u64 = 128;
const DEFAULT_BUNDLE_FETCH_MAX_MIB: u64 = 32;
const MAX_CONFIGURED_BUNDLE_CACHE_MIB: u64 = 512;
// A process crash anywhere around RunMicrovm cannot prove whether a VM was created. The reserved
// capacity therefore stays unavailable for the provider's full eight-hour VM wall plus skew;
// only an explicit KnownNoTarget response refunds early. Reclaiming a shorter lease could launch
// two physical VMs while charging the registry for one.
const TARGET_LEASE_MS: u64 = environment_lambda::MAX_DURATION_SECONDS * 1_000 + 5 * 60 * 1_000;
const TARGET_LIFETIME_MS: u64 = environment_lambda::MAX_DURATION_SECONDS * 1_000;
const TARGET_ATTEMPT_MS: u64 = 30_000;
// An exact client-token replay is useful only in the bounded crash-recovery window around the
// first RunMicrovm call. Allowing the first successful dispatch near the end of the eight-hour
// uncertainty lease would let that VM outlive the capacity reservation. Four minutes preserves
// several attempt takeovers while leaving at least one minute between the latest possible
// dispatch and the conservative provider-lifetime-plus-skew refund boundary.
const TARGET_DISPATCH_WINDOW_MS: u64 = 4 * 60 * 1_000;

mod cache;
mod config;
mod errors;
mod environment;
mod launcher;
mod plane;
mod ports;
mod status;
mod transfer;
mod validate;

pub use config::AwsEnvironmentConfig;
pub use environment::AwsMicrovmEnvironment;
pub use plane::AwsEnvironmentPlane;

#[allow(unused_imports)]
pub(crate) use cache::*;
#[allow(unused_imports)]
pub(crate) use config::*;
#[allow(unused_imports)]
pub(crate) use errors::*;
#[allow(unused_imports)]
pub(crate) use environment::*;
#[allow(unused_imports)]
pub(crate) use launcher::*;
#[allow(unused_imports)]
pub(crate) use plane::*;
#[allow(unused_imports)]
pub(crate) use ports::files::*;
#[allow(unused_imports)]
pub(crate) use status::*;
#[allow(unused_imports)]
pub(crate) use transfer::*;
#[allow(unused_imports)]
pub(crate) use validate::*;

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    fn preparation(session_id: &str, root_id: &str, digest: &str) -> PrepareSessionRequest {
        serde_json::from_value(serde_json::json!({
            "bindings": [{
                "binding_ref": format!("binding-{session_id}"),
                "bundle_digests": [digest]
            }],
            "bundles": [],
            "network": {"kind": "none"},
            "resources": {"max_output_bytes": 65536, "timeout_ms": 60000},
            "root_id": root_id,
            "session_id": session_id
        }))
        .unwrap()
    }

    fn managed_binding(session_id: &str, root_id: &str, digest: &str) -> SealedBinding {
        serde_json::from_value(serde_json::json!({
            "binding_id": format!("binding-{session_id}"),
            "bundle": {
                "bundle_digest": digest,
                "bytes": 1,
                "contract_digest": "b".repeat(64),
                "environment_name": "workspace",
                "execute_path": format!("/artifacts/{digest}/execute"),
                "object": {"bytes": 1, "object_id": "object-1", "sha256": digest},
                "required_env": [],
                "target": "linux-arm64",
                "tool_name": "fixture"
            },
            "capability": "fixture",
            "contract_digest": "b".repeat(64),
            "implementation_identity": "c".repeat(64),
            "environment_name": "workspace",
            "policy_digest": "d".repeat(64),
            "required_capabilities": ["execution"],
            "root_id": root_id,
            "session_id": session_id
        }))
        .unwrap()
    }

    fn materialization_lease(now_ms: u64) -> MaterializationLease {
        AcquireTarget {
            key: TargetKey::for_default_target("root-dispatch").unwrap(),
            spec: TargetSpec::new(
                ConnectorClass::None,
                "image-1",
                RESOURCE_CLASS,
                TARGET_MEMORY_MIB,
                "a".repeat(64),
                "b".repeat(64),
            )
            .unwrap(),
            reservation_id: "reservation-dispatch".into(),
            generation: "generation-dispatch".into(),
            launch_request: DurableLaunchRequest::new("sealed-launch").unwrap(),
            attempt_id: "attempt-dispatch".into(),
            attempt_duration_ms: TARGET_ATTEMPT_MS,
            generation_is_fenced: true,
            now_ms,
            lease_duration_ms: TARGET_LEASE_MS,
            target_lifetime_ms: TARGET_LIFETIME_MS,
            replace_after_loss: false,
        }
        .lease()
        .unwrap()
    }

    async fn streaming_response(body: Vec<u8>, declared_length: usize) -> reqwest::Response {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {declared_length}\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            for chunk in body.chunks(16 * 1024) {
                socket.write_all(chunk).await.unwrap();
            }
        });
        reqwest::Client::new()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap()
    }

    async fn stalled_streaming_response() -> reqwest::Response {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        reqwest::Client::new()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap()
    }

    #[test]
    fn target_specs_seal_physical_memory_and_exact_network_policy() {
        let cfg = AwsEnvironmentConfig {
            region: "us-east-1".into(),
            image: "image".into(),
            image_version: "1".into(),
            registry_table: "table".into(),
            max_materialized_mib: 1_024,
            bundle_cache_max_bytes: DEFAULT_BUNDLE_CACHE_MAX_MIB as usize * MIB,
            bundle_fetch_max_bytes: DEFAULT_BUNDLE_FETCH_MAX_MIB as usize * MIB,
            connectors: ConnectorCatalog::new(
                environment_core::connector::ConnectorRef::parse("none").unwrap(),
                environment_core::connector::ConnectorRef::parse("public").unwrap(),
                environment_core::connector::ConnectorRef::parse("allowlist").unwrap(),
            ),
            capability_signing_key_id: "key".into(),
            egress_gateway_authority: GatewayAuthority::parse("10.0.0.10:8443").unwrap(),
        };
        let resources: ResourceCeiling = serde_json::from_value(serde_json::json!({
            "max_output_bytes": 1024,
            "timeout_ms": 1000
        }))
        .unwrap();
        let none = target_spec(&cfg, &resources, &NetworkCeiling::None, RESOURCE_CLASS).unwrap();
        let public =
            target_spec(&cfg, &resources, &NetworkCeiling::Public, RESOURCE_CLASS).unwrap();
        assert_eq!(none.materialized_mib, 1_024);
        assert_ne!(none.network_policy_digest, public.network_policy_digest);
        assert_ne!(none.digest(), public.digest());
    }

    #[test]
    fn provider_status_projection_never_calls_a_transitional_or_future_state_running() {
        use aws_sdk_lambdamicrovms::types::MicrovmState;

        assert_eq!(
            sandbox_state_from_provider(&MicrovmState::Running).unwrap(),
            SandboxState::Running
        );
        assert!(sandbox_state_from_provider(&MicrovmState::Suspending).is_err());
        let terminating = sandbox_state_from_provider(&MicrovmState::Terminating).unwrap_err();
        assert_eq!(terminating.code, EnvironmentErrorCode::TemporarilyUnavailable);
        assert!(terminating.retryable);
        let future = sandbox_state_from_provider(&MicrovmState::from("FUTURE_STATE")).unwrap_err();
        assert_eq!(future.code, EnvironmentErrorCode::TemporarilyUnavailable);
        assert!(future.retryable);
    }

    #[test]
    fn physical_loss_after_submit_dispatch_is_operation_unknown_not_a_replacement_signal() {
        let classified = classify_submit_delivery_error(error(
            EnvironmentErrorCode::SandboxGone,
            false,
            "physical generation disappeared",
        ));
        assert_eq!(classified.code, EnvironmentErrorCode::OperationUnknown);
        assert!(!classified.retryable);

        let pre_dispatch = error(
            EnvironmentErrorCode::CapabilityUnavailable,
            false,
            "bundle was not installed",
        );
        let classified = classify_submit_delivery_error(pre_dispatch);
        assert_eq!(classified.code, EnvironmentErrorCode::CapabilityUnavailable);
        assert!(!classified.retryable);
        assert_eq!(classified.message.as_str(), "bundle was not installed");
    }

    #[test]
    fn verified_bundle_cache_is_lru_and_only_active_borrowers_pin_bytes() {
        let digest = "a".repeat(64);
        let mut cache = PreparationCache::with_limit(3);
        cache
            .install(
                preparation("session-1", "root-1", &digest),
                "preparation-1".into(),
                HashMap::from([(digest.clone(), Arc::new(vec![1, 2, 3]))]),
            )
            .unwrap();
        cache
            .install(
                preparation("session-2", "root-1", &digest),
                "preparation-2".into(),
                HashMap::new(),
            )
            .unwrap();
        assert_eq!(cache.bundles.bundle_bytes, 3);
        assert_eq!(cache.bundle(&digest).unwrap().as_slice(), &[1, 2, 3]);

        assert!(!cache.purge_root_page("root-1", 1));
        assert!(cache.bundle(&digest).is_some());
        assert!(cache.purge_root_page("root-1", 1));
        // Preparation metadata no longer pins resident bytes. Purge need not synchronously flush
        // a content-addressed cache entry, but the next admission may evict it immediately.
        let borrowed = cache.bundle(&digest).unwrap();
        let replacement = "b".repeat(64);
        let error = cache
            .install(
                preparation("session-3", "root-2", &replacement),
                "preparation-3".into(),
                HashMap::from([(replacement.clone(), Arc::new(vec![4, 5, 6]))]),
            )
            .unwrap_err();
        assert_eq!(error.code, EnvironmentErrorCode::ResourceExhausted);
        assert_eq!(cache.bundles.bundle_bytes, 3);
        drop(borrowed);

        cache
            .install(
                preparation("session-3", "root-2", &replacement),
                "preparation-3".into(),
                HashMap::from([(replacement.clone(), Arc::new(vec![4, 5, 6]))]),
            )
            .unwrap();
        assert!(cache.bundle(&digest).is_none());
        assert_eq!(cache.bundle(&replacement).unwrap().as_slice(), &[4, 5, 6]);
        assert_eq!(cache.bundles.bundle_bytes, 3);
    }

    #[test]
    fn preparation_metadata_is_bounded_lru_and_eviction_requires_reprepare() {
        let digest = "a".repeat(64);
        let first = preparation("session-1", "root-1", &digest);
        let metadata_bytes = serde_jcs::to_vec(&first).unwrap().len();
        let mut cache = PreparationCache::with_limits(3, metadata_bytes * 2, 2);
        cache
            .install(
                first,
                "preparation-1".into(),
                HashMap::from([(digest.clone(), Arc::new(vec![1, 2, 3]))]),
            )
            .unwrap();
        cache
            .install(
                preparation("session-2", "root-2", &digest),
                "preparation-2".into(),
                HashMap::new(),
            )
            .unwrap();

        // Touch session 1 after session 2 was installed, making session 2 the cold candidate.
        assert!(cache.get("session-1").is_some());
        cache
            .install(
                preparation("session-3", "root-3", &digest),
                "preparation-3".into(),
                HashMap::new(),
            )
            .unwrap();
        assert!(cache.get("session-1").is_some());
        assert!(cache.get("session-2").is_none());
        assert!(cache.get("session-3").is_some());
        assert_eq!(cache.store.sessions.len(), 2);
        assert!(cache.store.preparation_bytes <= metadata_bytes * 2);
        assert!(!cache.store.root_sessions.contains_key("root-2"));
    }

    #[test]
    fn cold_bundle_fetch_admission_bounds_cache_and_all_in_flight_bytes() {
        let reserved = Arc::new(StdMutex::new(BundleFetchInFlight::default()));
        let cache_limit = DEFAULT_BUNDLE_CACHE_MAX_MIB as usize * MIB;
        let fetch_limit = DEFAULT_BUNDLE_FETCH_MAX_MIB as usize * MIB;
        let first = BundleFetchReservation::admit(
            reserved.clone(),
            cache_limit - 8,
            1,
            8,
            1,
            cache_limit,
            fetch_limit,
        )
        .unwrap();
        assert_eq!(reserved.lock().unwrap().bytes, 8);

        let error = BundleFetchReservation::admit(
            reserved.clone(),
            cache_limit - 8,
            1,
            1,
            1,
            cache_limit,
            fetch_limit,
        )
        .unwrap_err();
        assert_eq!(error.code, EnvironmentErrorCode::ResourceExhausted);
        assert!(error.retryable);
        assert_eq!(reserved.lock().unwrap().bytes, 8);

        drop(first);
        assert_eq!(reserved.lock().unwrap().bytes, 0);
        let retry = BundleFetchReservation::admit(
            reserved.clone(),
            cache_limit - 8,
            1,
            8,
            1,
            cache_limit,
            fetch_limit,
        )
        .unwrap();
        drop(retry);
        assert_eq!(reserved.lock().unwrap().bytes, 0);

        let first = BundleFetchReservation::admit(
            reserved.clone(),
            0,
            0,
            fetch_limit,
            1,
            cache_limit,
            fetch_limit,
        )
        .unwrap();
        let error =
            BundleFetchReservation::admit(reserved.clone(), 0, 0, 1, 1, cache_limit, fetch_limit)
                .unwrap_err();
        assert_eq!(
            error.details.get("scope"),
            Some(&serde_json::Value::String("environment_bundle_fetch_bytes".into()))
        );
        drop(first);
    }

    #[test]
    fn cache_loss_requires_a_fresh_fetch_before_any_tool_effect() {
        let digest = "a".repeat(64);
        let mut cache = PreparationCache::default();
        let error = cache
            .install(
                preparation("session-1", "root-1", &digest),
                "preparation-1".into(),
                HashMap::new(),
            )
            .unwrap_err();
        assert_eq!(error.code, EnvironmentErrorCode::CapabilityUnavailable);
        assert!(!error.retryable);
        assert!(cache.get("session-1").is_none());
    }

    #[test]
    fn prepared_session_public_seal_is_immutable_but_fresh_fetches_may_replay() {
        let digest = "a".repeat(64);
        let mut cache = PreparationCache::default();
        let request = preparation("session-1", "root-1", &digest);
        cache
            .install(
                request.clone(),
                "preparation-1".into(),
                HashMap::from([(digest.clone(), Arc::new(vec![1, 2, 3]))]),
            )
            .unwrap();
        cache
            .install(request.clone(), "preparation-1".into(), HashMap::new())
            .unwrap();
        let conflict = cache
            .install(request, "preparation-changed".into(), HashMap::new())
            .unwrap_err();
        assert_eq!(conflict.code, EnvironmentErrorCode::BindingConflict);
    }

    #[test]
    fn preparation_seals_secret_names_but_not_the_refreshable_bearer() {
        let digest = "a".repeat(64);
        let mut first = preparation("session-1", "root-1", &digest);
        first.secret_capability = serde_json::from_value(serde_json::json!({
            "capability_ref": "secret-capability-1",
            "env_names": ["DATABASE_URL", "OPENAI_API_KEY"],
            "expires_at_ms": 1000
        }))
        .unwrap();
        let mut refreshed = first.clone();
        refreshed.secret_capability.as_mut().unwrap().capability_ref =
            "secret-capability-2".parse().unwrap();
        refreshed.secret_capability.as_mut().unwrap().expires_at_ms =
            std::num::NonZeroU64::new(2_000).unwrap();
        assert_eq!(
            preparation_public_projection(&first).unwrap(),
            preparation_public_projection(&refreshed).unwrap()
        );

        refreshed.secret_capability.as_mut().unwrap().env_names =
            vec!["DATABASE_URL".parse().unwrap()];
        assert_ne!(
            preparation_public_projection(&first).unwrap(),
            preparation_public_projection(&refreshed).unwrap()
        );
    }

    #[test]
    fn preparation_binding_projection_is_exactly_root_session_and_descriptor_scoped() {
        let digest = "a".repeat(64);
        let request = preparation("session-1", "root-1", &digest);
        let prepared = &request.bindings[0];
        let binding = managed_binding("session-1", "root-1", &digest);
        assert_eq!(
            validate_prepared_binding_projection(prepared, &binding, "root-1", "session-1")
                .unwrap()
                .digest,
            digest
        );

        let wrong_session = managed_binding("session-2", "root-1", &digest);
        assert_eq!(
            validate_prepared_binding_projection(prepared, &wrong_session, "root-1", "session-1")
                .unwrap_err()
                .code,
            EnvironmentErrorCode::BindingConflict
        );
        let mut wrong_digest = prepared.clone();
        wrong_digest.bundle_digests = vec!["e".repeat(64).parse().unwrap()];
        assert_eq!(
            validate_prepared_binding_projection(&wrong_digest, &binding, "root-1", "session-1")
                .unwrap_err()
                .code,
            EnvironmentErrorCode::BindingConflict
        );

        let mut inconsistent_object = binding.clone();
        inconsistent_object.bundle.as_mut().unwrap().object.bytes = 2;
        assert_eq!(
            validate_managed_binding(&inconsistent_object)
                .unwrap_err()
                .code,
            EnvironmentErrorCode::BindingConflict
        );

        let mut exact_limit = binding.clone();
        let bytes = NonZeroU64::new(brain_protocol::MAX_TOOL_BUNDLE_BYTES as u64).unwrap();
        exact_limit.bundle.as_mut().unwrap().bytes = bytes;
        exact_limit.bundle.as_mut().unwrap().object.bytes = bytes.get();
        validate_managed_binding(&exact_limit).unwrap();

        let mut oversized = exact_limit;
        let bytes = NonZeroU64::new(bytes.get() + 1).unwrap();
        oversized.bundle.as_mut().unwrap().bytes = bytes;
        oversized.bundle.as_mut().unwrap().object.bytes = bytes.get();
        assert_eq!(
            validate_managed_binding(&oversized).unwrap_err().code,
            EnvironmentErrorCode::BindingConflict
        );

        let mut repeated_env = binding.clone();
        repeated_env.bundle.as_mut().unwrap().required_env =
            vec!["TOKEN".parse().unwrap(), "TOKEN".parse().unwrap()];
        assert_eq!(
            validate_managed_binding(&repeated_env).unwrap_err().code,
            EnvironmentErrorCode::BindingConflict
        );

        let first = validate_prepared_binding_projection(prepared, &binding, "root-1", "session-1")
            .unwrap();
        let mut conflicting_descriptor = binding.clone();
        conflicting_descriptor.bundle.as_mut().unwrap().tool_name = "other".parse().unwrap();
        let second = validate_prepared_binding_projection(
            prepared,
            &conflicting_descriptor,
            "root-1",
            "session-1",
        )
        .unwrap();
        let mut required = HashMap::new();
        merge_validated_prepared_bundle(&mut required, first).unwrap();
        assert_eq!(
            merge_validated_prepared_bundle(&mut required, second)
                .unwrap_err()
                .code,
            EnvironmentErrorCode::BindingConflict
        );
    }

    #[test]
    fn short_lived_bundle_fetch_authorities_are_not_retained_in_session_cache() {
        const URL_SECRET: &str = "presigned-url-secret";
        const HEADER_SECRET: &str = "authorization-header-secret";
        let digest = "a".repeat(64);
        let mut request = preparation("session-1", "root-1", &digest);
        request.bundles = serde_json::from_value(serde_json::json!([{
            "bundle_digest": digest,
            "url": format!("https://objects.example.test/bundle?signature={URL_SECRET}"),
            "headers": {"Authorization": HEADER_SECRET},
            "expires_at_ms": 123456,
            "max_bytes": 4096
        }]))
        .unwrap();

        let cached = cacheable_preparation(request);
        assert!(cached.bundles.is_empty());
        let encoded = serde_json::to_string(&cached).unwrap();
        assert!(!encoded.contains(URL_SECRET));
        assert!(!encoded.contains(HEADER_SECRET));
        assert!(encoded.contains("binding-session-1"));
    }

    #[test]
    fn additional_target_cannot_widen_root_resource_or_network_seals() {
        let root: ResourceCeiling = serde_json::from_value(serde_json::json!({
            "max_output_bytes": 65536,
            "timeout_ms": 60000
        }))
        .unwrap();
        let narrower: ResourceCeiling = serde_json::from_value(serde_json::json!({
            "max_output_bytes": 32768,
            "timeout_ms": 30000
        }))
        .unwrap();
        assert!(validate_resource_ceiling_subset(&narrower, &root).is_ok());
        let wider: ResourceCeiling = serde_json::from_value(serde_json::json!({
            "max_output_bytes": 65537,
            "timeout_ms": 60000
        }))
        .unwrap();
        assert_eq!(
            validate_resource_ceiling_subset(&wider, &root)
                .unwrap_err()
                .code,
            EnvironmentErrorCode::GenerationConflict
        );

        let root_network: NetworkCeiling = serde_json::from_value(serde_json::json!({
            "kind": "allowlist",
            "destinations": [
                {"protocol": "tls", "host": "*.example.com", "ports": [443]},
                {"protocol": "tcp", "cidr": "8.8.8.0/24", "ports": [443, 8443]}
            ]
        }))
        .unwrap();
        let narrow_network: NetworkCeiling = serde_json::from_value(serde_json::json!({
            "kind": "allowlist",
            "destinations": [
                {"protocol": "tls", "host": "objects.example.com", "ports": [443]},
                {"protocol": "tcp", "cidr": "8.8.8.8/32", "ports": [443]}
            ]
        }))
        .unwrap();
        assert!(network_ceiling_is_subset(&narrow_network, &root_network));
        assert!(!network_ceiling_is_subset(
            &NetworkCeiling::Public,
            &root_network
        ));
    }

    #[test]
    fn additional_control_port_never_overloads_default_lifecycle() {
        let default: SandboxTarget = serde_json::from_value(serde_json::json!({
            "binding_ref": "binding-default",
            "kind": "default",
            "root_id": "root-1",
            "session_id": "session-1"
        }))
        .unwrap();
        let error = require_additional_target(&default).unwrap_err();
        assert_eq!(error.code, EnvironmentErrorCode::InvalidRequest);

        let additional: SandboxTarget = serde_json::from_value(serde_json::json!({
            "binding_ref": "binding-additional",
            "kind": "additional",
            "root_id": "root-1",
            "sandbox_id": "sandbox-1",
            "session_id": "session-1"
        }))
        .unwrap();
        require_additional_target(&additional).unwrap();
    }

    #[test]
    fn managed_tool_input_is_inline_and_bounded_before_materialization() {
        let small = serde_json::from_value(serde_json::json!({
            "kind": "inline",
            "value": {"prompt": "hello"}
        }))
        .unwrap();
        assert!(validate_inline_input(&small).is_ok());

        let wrong_kind = brain_protocol::environment::OperationInput {
            kind: serde_json::json!("object"),
            value: serde_json::json!({}),
        };
        assert_eq!(
            validate_inline_input(&wrong_kind).unwrap_err().code,
            EnvironmentErrorCode::InvalidRequest
        );
        let empty = brain_protocol::environment::OperationInput {
            kind: serde_json::json!("inline"),
            value: serde_json::json!(""),
        };
        let framing = serde_jcs::to_vec(&empty).unwrap().len();
        let exact = brain_protocol::environment::OperationInput {
            kind: serde_json::json!("inline"),
            value: serde_json::json!(
                "x".repeat(brain_protocol::MAX_MANAGED_TOOL_INPUT_BYTES - framing)
            ),
        };
        assert_eq!(
            serde_jcs::to_vec(&exact).unwrap().len(),
            brain_protocol::MAX_MANAGED_TOOL_INPUT_BYTES
        );
        assert!(validate_inline_input(&exact).is_ok());

        let oversized = brain_protocol::environment::OperationInput {
            kind: serde_json::json!("inline"),
            value: serde_json::json!(
                "x".repeat(brain_protocol::MAX_MANAGED_TOOL_INPUT_BYTES - framing + 1)
            ),
        };
        assert_eq!(
            serde_jcs::to_vec(&oversized).unwrap().len(),
            brain_protocol::MAX_MANAGED_TOOL_INPUT_BYTES + 1
        );
        assert_eq!(
            validate_inline_input(&oversized).unwrap_err().code,
            EnvironmentErrorCode::InvalidRequest
        );
    }

    #[test]
    fn capacity_errors_are_typed_and_expose_retry_without_a_fallback() {
        let error = materialization_error(MaterializationError::Capacity {
            scope: "plane_materialized_memory_mib".into(),
            retry_after_ms: 1_000,
            message: "full".into(),
        });
        assert_eq!(error.code, EnvironmentErrorCode::ResourceExhausted);
        assert!(error.retryable);
        assert_eq!(error.details["retry_after_ms"], 1_000);
        assert_eq!(error.details["scope"], "plane_materialized_memory_mib");
    }

    #[test]
    fn delayed_provider_dispatch_never_outlives_the_capacity_fence() {
        let fresh = materialization_lease(1_000);
        let deadline = launch_dispatch_deadline(&fresh).unwrap();
        assert_eq!(deadline, 1_000 + TARGET_DISPATCH_WINDOW_MS);
        assert!(admit_provider_dispatch(&fresh, deadline, deadline).is_ok());
        assert!(matches!(
            admit_provider_dispatch(&fresh, deadline, deadline + 1),
            Err(LaunchError::KnownNoTarget(_))
        ));

        let mut recovery = fresh;
        recovery.recovery_attempt = true;
        assert!(matches!(
            admit_provider_dispatch(&recovery, deadline, deadline + 1),
            Err(LaunchError::OutcomeUnknown(_))
        ));
        assert!(
            recovery.lease_expires_at_ms.saturating_sub(deadline) >= TARGET_LIFETIME_MS,
            "the latest admitted provider dispatch must expire before capacity can be reused"
        );
    }

    #[test]
    fn transfer_authorities_require_sealed_https_without_url_credentials() {
        assert!(
            validate_https_authority_url("https://objects.example/a?X-Amz-Signature=opaque")
                .is_ok()
        );
        for invalid_url in [
            "http://objects.example/a",
            "https://user:secret@objects.example/a",
            "https://objects.example/a#fragment",
            "/relative/path",
        ] {
            let error = validate_https_authority_url(invalid_url).unwrap_err();
            assert_eq!(error.code, EnvironmentErrorCode::InvalidRequest);
            assert!(!error.retryable);
        }
    }

    #[test]
    fn transfer_authorities_cannot_override_transport_routing_headers() {
        let http = reqwest::Client::new();
        for name in [
            "Host",
            "Content-Length",
            "Connection",
            "Keep-Alive",
            "Proxy-Authenticate",
            "Proxy-Authorization",
            "Proxy-Connection",
            "TE",
            "Trailer",
            "Transfer-Encoding",
            "Upgrade",
        ] {
            let error = apply_authority_headers(
                http.get("https://objects.example.test/object"),
                std::iter::once((name, "opaque")),
            )
            .unwrap_err();
            assert_eq!(error.code, EnvironmentErrorCode::InvalidRequest, "{name}");
            assert!(!error.retryable, "{name}");
        }
        assert!(
            apply_authority_headers(
                http.get("https://objects.example.test/object"),
                std::iter::once(("Authorization", "Bearer opaque")),
            )
            .is_ok()
        );
    }

    #[test]
    fn source_authorities_refresh_but_export_destination_identity_conflicts() {
        let file_write = |transfer_id: &str, token: &str| -> SandboxFileWriteRequest {
            serde_json::from_value(serde_json::json!({
                "operation_id": "file-write-refresh-1",
                "request_digest": "0".repeat(64),
                "expected_generation": "generation-1",
                "overwrite": false,
                "path": "/workspace/input.bin",
                "source": {
                    "kind": "object",
                    "fetch": {
                        "expires_at_ms": 9_000_000_000_000_u64,
                        "headers": {"Authorization": format!("Bearer {token}")},
                        "max_bytes": 1024,
                        "method": "GET",
                        "object_id": "object-source-1",
                        "transfer_id": transfer_id,
                        "url": format!("https://objects.example.test/input?token={token}")
                    },
                    "object": {
                        "bytes": 7,
                        "object_id": "object-source-1",
                        "sha256": "a".repeat(64)
                    }
                },
                "target": {
                    "binding_ref": "binding-1",
                    "kind": "default",
                    "root_id": "root-1",
                    "session_id": "session-1"
                }
            }))
            .unwrap()
        };
        assert_eq!(
            sandbox_file_write_request_digest(&file_write("fetch-1", "old")),
            sandbox_file_write_request_digest(&file_write("fetch-2", "fresh")),
            "the immutable ObjectReference, not a refreshed GET reservation, owns source identity"
        );

        let copy = |direction: &str, transfer_id: &str, token: &str| -> SandboxCopyRequest {
            let importing = direction == "import";
            serde_json::from_value(serde_json::json!({
                "direction": direction,
                "expected_generation": "generation-1",
                "object": importing.then(|| serde_json::json!({
                    "bytes": 7,
                    "object_id": "object-source-1",
                    "sha256": "a".repeat(64)
                })),
                "operation_id": format!("copy-{direction}-refresh-1"),
                "overwrite": false,
                "path": "/workspace/input.bin",
                "request_digest": "0".repeat(64),
                "target": {
                    "binding_ref": "binding-1",
                    "kind": "default",
                    "root_id": "root-1",
                    "session_id": "session-1"
                },
                "transfer": {
                    "expires_at_ms": 9_000_000_000_000_u64,
                    "headers": {"Authorization": format!("Bearer {token}")},
                    "max_bytes": 1024,
                    "method": if importing { "GET" } else { "PUT" },
                    "object_id": if importing { "object-source-1" } else { "object-destination-1" },
                    "transfer_id": transfer_id,
                    "url": format!("https://objects.example.test/{direction}?token={token}")
                }
            }))
            .unwrap()
        };
        assert_eq!(
            sandbox_copy_request_digest(&copy("import", "fetch-1", "old")),
            sandbox_copy_request_digest(&copy("import", "fetch-2", "fresh")),
            "import GET reservations are refreshable"
        );
        assert_ne!(
            sandbox_copy_request_digest(&copy("export", "upload-1", "old")),
            sandbox_copy_request_digest(&copy("export", "upload-2", "fresh")),
            "export transfer_id names the pending destination and cannot refresh"
        );
    }

    #[test]
    fn object_write_projection_never_forwards_storage_authority_to_the_guest() {
        const URL_SECRET: &str = "presigned-object-url-secret";
        const HEADER_SECRET: &str = "presigned-object-header-secret";
        let mut request: SandboxFileWriteRequest = serde_json::from_value(serde_json::json!({
            "operation_id": "file-write-1",
            "request_digest": "0".repeat(64),
            "expected_generation": "generation-1",
            "overwrite": false,
            "path": "/workspace/input.bin",
            "source": {
                "kind": "object",
                "fetch": {
                    "expires_at_ms": u64::MAX,
                    "headers": {"Authorization": HEADER_SECRET},
                    "max_bytes": 1024,
                    "method": "GET",
                    "object_id": "object-1",
                    "transfer_id": "transfer-1",
                    "url": format!("https://objects.example.test/input?signature={URL_SECRET}")
                },
                "object": {
                    "bytes": 7,
                    "object_id": "object-1",
                    "sha256": "a".repeat(64)
                }
            },
            "target": {
                "binding_ref": "binding-1",
                "kind": "default",
                "root_id": "root-1",
                "session_id": "session-1"
            }
        }))
        .unwrap();
        request.request_digest = sandbox_file_write_request_digest(&request);
        let encoded = serde_json::to_string(&project_guest_file_write(request)).unwrap();
        assert!(!encoded.contains(URL_SECRET));
        assert!(!encoded.contains(HEADER_SECRET));
        assert!(!encoded.contains("transfer-1"));
        assert!(encoded.contains("object-1"));
        assert!(encoded.contains("installed_object"));
        assert!(encoded.contains("file-write-1"));
    }

    #[tokio::test]
    async fn object_staging_streams_beyond_inline_size_and_enforces_the_bound() {
        let body = vec![0x5a; 2 * 1024 * 1024];
        let expected = hex::encode(Sha256::digest(&body));
        let response = streaming_response(body.clone(), body.len()).await;
        let staged = stage_response(response, body.len() as u64, now_ms() + 60_000)
            .await
            .unwrap();
        assert_eq!(staged.bytes, body.len() as u64);
        assert_eq!(staged.sha256, expected);
        assert_eq!(
            std::fs::metadata(staged.file.path()).unwrap().len(),
            body.len() as u64
        );

        let response = streaming_response(vec![0_u8; 2], 2).await;
        let error = match stage_response(response, 1, now_ms() + 60_000).await {
            Ok(_) => panic!("over-bound response must be rejected"),
            Err(error) => error,
        };
        assert_eq!(error.code, EnvironmentErrorCode::ResourceExhausted);
        assert!(!error.retryable);

        let response = streaming_response(Vec::new(), 0).await;
        let staged = stage_response(response, 0, now_ms() + 60_000)
            .await
            .expect("an immutable empty object is valid");
        assert_eq!(staged.bytes, 0);
        assert_eq!(staged.sha256, hex::encode(Sha256::digest([])));
        assert_eq!(std::fs::metadata(staged.file.path()).unwrap().len(), 0);

        let response = stalled_streaming_response().await;
        let expired = match tokio::time::timeout(
            Duration::from_millis(250),
            stage_response(response, 1, now_ms() + 25),
        )
        .await
        .expect("a stalled body must be bounded by its authority deadline")
        {
            Err(error) => error,
            Ok(_) => panic!("a body that did not arrive before expiry must be rejected"),
        };
        assert!(!expired.retryable);
    }
}
