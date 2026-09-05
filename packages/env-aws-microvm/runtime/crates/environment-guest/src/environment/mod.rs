//! Physical-generation implementation of Brain's canonical receipt protocol.

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use environment_core::connector::ConnectorClass;
use environment_core::files::{LiveFileEntry, LiveFileError, LiveFileKind, LiveFiles};
use environment_core::operation::{OperationError, OperationRegistry, OperationState, Reservation};
use environment_core::resources::{ResourceRequest, ResourceSupport};
use environment_policy::guest_env::{
    environment_name_is_valid, reserved_tool_environment, secret_material_fits,
};
use environment_wire::network_ceiling_is_subset;
use environment_wire::{
    AcknowledgeTerminalRequest, Acknowledgement, ArtifactTarget, BundleDescriptor, CancelRequest,
    CancellationReceipt, Digest, EnvironmentError, EnvironmentErrorCode, FileEntry, FileEntryKind,
    NetworkCeiling, ObserveRequest, OperationEnvelope, OperationObservation, OperationRef,
    OperationState as ContractOperationState, ResourceCeiling, SandboxCopyResult,
    SandboxExecutionRequest, SandboxFileRequest, SandboxFileWriteResult, SandboxTarget,
    SealedBinding, SubmitReceipt, SubmitRequest, TargetKind, TargetReceipt, TerminalOutcome,
    TerminalResult, WriteStdinReceipt, WriteStdinRequest,
};
use environment_wire::{
    ENVIRONMENT_CONTRACT_DIGEST, operation_request_digest, sandbox_execution_request_digest,
    terminal_inline_fits, terminal_result_digest, write_stdin_request_digest,
};
use environment_wire::{
    FileEffectIdentity, FileEffectKind, FileEffectReservation, FileEffectStoredResult,
    GuestFileWriteRequest, GuestFileWriteSource, InstallBindingRequest, InstallBundleMetadata,
    InstallObjectMetadata, InstallReceipt, InstallSecretsRequest, MAX_INLINE_FILE_BYTES,
    RunPayload, TargetRuntimeStatus,
};
use environment_wire::{
    SandboxFileContent, SandboxFileList, SandboxFileListRequest, SandboxSearchRequest,
};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::{Mutex, Notify, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroize as _;

use crate::acks::{AcknowledgementStore, SubmissionFence};
use crate::config::{
    Config, MANAGED_BINDING_UID_MIN, MANAGED_BINDING_UID_SPAN, MAX_CONCURRENT_OPERATIONS,
    MAX_OPERATION_OUTPUT_BYTES, MAX_OPERATION_TIMEOUT_MS, MAX_PREPARED_BINDINGS,
    MAX_RETAINED_OPERATIONS, MAX_RETAINED_STDIN_WRITES, MAX_RETAINED_TERMINAL_BYTES,
    MAX_STDIN_REPLAY_WAIT_MS, MAX_TARGET_LIFETIME_MS, MAX_WAIT_MS, ToolIdentity, wall_ms,
};
use crate::errors::{
    ack_store_error, environment_error, file_effect_store_error, generation_conflict, invalid,
    operation_error, stdin_conflict, unavailable,
};
use crate::file_effects::{EffectReservation, FileEffectStore};
use crate::process::{
    BundleExecution, InteractiveControl, ShellExecution, execute_bundle, execute_shell,
};

// Brain reserves 4 KiB above the canonical inline value for the terminal outcome, digest, timing,
// operation locator, and target receipt. Reserve that complete envelope before starting an effect
// so terminal retention cannot leave an operation permanently `running` after its child exits.
const TERMINAL_ENVELOPE_BYTES: usize = environment_wire::MAX_TOOL_TERMINAL_INLINE_BYTES + 4 * 1024;
const FILE_EFFECT_LOCK_SHARDS: usize = 64;

mod files;
mod install;
mod operations;
mod receipts;
mod stdin;
mod target;

#[allow(unused_imports)]
pub(crate) use files::*;
#[allow(unused_imports)]
pub(crate) use install::*;
#[allow(unused_imports)]
pub(crate) use operations::*;
#[allow(unused_imports)]
pub(crate) use receipts::*;
#[allow(unused_imports)]
pub(crate) use stdin::*;
#[allow(unused_imports)]
pub(crate) use target::*;

/// One Environment guest per physical sandbox generation: a thin façade over five independent state
/// domains, each owning its own locks. Cross-domain flows (submit, shutdown) compose them here.
pub struct Environment {
    pub cfg: Config,
    pub(crate) target: TargetState,
    pub(crate) artifacts: Artifacts,
    pub(crate) operations: Operations,
    pub(crate) effects: FileEffects,
    pub(crate) stdin: StdinState,
    pub(crate) files: LiveFiles,
    pub(crate) shutdown: CancellationToken,
}

/// The armed physical generation, set exactly once per process by `arm`.
pub(crate) struct TargetState {
    pub(crate) armed: RwLock<Option<ArmedTarget>>,
}

/// Immutable installed artifacts: bundle bytes on disk, sealed bindings, per-binding kernel
/// identities, and session secret material.
pub(crate) struct Artifacts {
    pub(crate) layers: RwLock<HashMap<String, PathBuf>>,
    pub(crate) bundles: RwLock<HashMap<String, (BundleDescriptor, PathBuf)>>,
    pub(crate) bindings: RwLock<HashMap<String, InstalledBinding>>,
    pub(crate) identities: Mutex<BindingIdentityRegistry>,
    pub(crate) secrets: RwLock<HashMap<String, SessionSecrets>>,
}

/// Operation admission, retention, and acknowledgement state.
pub(crate) struct Operations {
    pub(crate) book: Mutex<OperationBook>,
    pub(crate) acknowledgements: Arc<AcknowledgementStore>,
    pub(crate) slots: Arc<Semaphore>,
}

/// Durable two-phase file-effect state plus its shard locks.
pub(crate) struct FileEffects {
    pub(crate) store: Arc<FileEffectStore>,
    pub(crate) locks: [Mutex<()>; FILE_EFFECT_LOCK_SHARDS],
}

/// Idempotent interactive stdin write records.
pub(crate) struct StdinState {
    pub(crate) book: Mutex<StdinBook>,
}

impl Environment {
    pub fn new(cfg: Config) -> anyhow::Result<Arc<Self>> {
        std::fs::create_dir_all(&cfg.workspace)?;
        std::fs::create_dir_all(&cfg.state_dir)?;
        std::fs::create_dir_all(&cfg.tool_dir)?;
        std::fs::create_dir_all(&cfg.object_dir)?;
        let acknowledgement_dir = cfg.state_dir.join("ops");
        std::fs::create_dir_all(&acknowledgement_dir)?;
        let acknowledgements = Arc::new(AcknowledgementStore::open(
            &acknowledgement_dir.join("acknowledged.jsonl"),
        )?);
        let file_effects = Arc::new(FileEffectStore::open(
            &acknowledgement_dir.join("file-effects.jsonl"),
        )?);
        let files = LiveFiles::new(&cfg.workspace, cfg.state_dir.join("file-staging"))?;
        Ok(Arc::new(Self {
            cfg,
            target: TargetState {
                armed: RwLock::new(None),
            },
            artifacts: Artifacts {
                layers: RwLock::new(HashMap::new()),
                bundles: RwLock::new(HashMap::new()),
                bindings: RwLock::new(HashMap::new()),
                identities: Mutex::new(BindingIdentityRegistry::production()),
                secrets: RwLock::new(HashMap::new()),
            },
            operations: Operations {
                book: Mutex::new(OperationBook {
                    registry: OperationRegistry::new(
                        MAX_RETAINED_OPERATIONS,
                        MAX_RETAINED_TERMINAL_BYTES,
                    ),
                    metadata: HashMap::new(),
                }),
                acknowledgements,
                slots: Arc::new(Semaphore::new(MAX_CONCURRENT_OPERATIONS)),
            },
            effects: FileEffects {
                store: file_effects,
                locks: std::array::from_fn(|_| Mutex::new(())),
            },
            stdin: StdinState {
                book: Mutex::new(StdinBook {
                    records: HashMap::new(),
                }),
            },
            files,
            shutdown: CancellationToken::new(),
        }))
    }
}

impl Environment {
    pub async fn shutdown(&self) {
        self.shutdown.cancel();
        // Refuse every queued operation before collecting active cancellations. Existing tasks
        // retain their permits until their process group has been killed, reaped, and projected
        // terminal; queued tasks never spawn a child after shutdown begins.
        self.operations.slots.close();
        let cancellations = self
            .operations
            .book
            .lock()
            .await
            .metadata
            .values()
            .map(|meta| meta.cancellation.clone())
            .collect::<Vec<_>>();
        for cancellation in cancellations {
            cancellation.cancel();
        }
        // `terminate_process_group` allows two seconds for TERM and one second for the final KILL
        // reap. Keep the supervisor (and therefore CAP_KILL) alive across that complete bounded
        // cleanup path instead of dropping Tokio while cross-UID children can still be running.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(4);
        while self.operations.slots.available_permits() != MAX_CONCURRENT_OPERATIONS
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

async fn blocking_environment<T, F>(work: F) -> Result<T, EnvironmentError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, EnvironmentError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| unavailable("bounded file worker failed"))?
}

#[cfg(test)]
mod tests;
