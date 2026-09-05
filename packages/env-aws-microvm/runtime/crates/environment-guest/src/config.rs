//! Environment-driven guest configuration and hard admission bounds.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

pub const ENV_LISTEN: &str = "ENVIRONMENT_LISTEN";
pub const ENV_WORKSPACE: &str = "ENVIRONMENT_WORKSPACE";
pub const ENV_STATE_DIR: &str = "ENVIRONMENT_STATE_DIR";
pub const ENV_TOOL_DIR: &str = "ENVIRONMENT_TOOL_DIR";
pub const ENV_TOOL_RUNNER: &str = "ENVIRONMENT_TOOL_RUNNER";
pub const ENV_TOOL_BOUNDARY_LIBRARY: &str = "ENVIRONMENT_TOOL_BOUNDARY_LIBRARY";
pub const ENV_SUPERVISOR_UID: &str = "ENVIRONMENT_SUPERVISOR_UID";
pub const ENV_TOOL_UID: &str = "ENVIRONMENT_TOOL_UID";
pub const ENV_TOOL_GID: &str = "ENVIRONMENT_TOOL_GID";

pub const MAX_CONCURRENT_OPERATIONS: usize = 64;
pub const MAX_RETAINED_OPERATIONS: usize = 1_024;
pub const MAX_RETAINED_STDIN_WRITES: usize = 4_096;
pub const MAX_RETAINED_TERMINAL_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_OPERATION_OUTPUT_BYTES: u64 = environment_wire::MAX_TOOL_TERMINAL_INLINE_BYTES as u64;
pub const MAX_OPERATION_TIMEOUT_MS: u64 = MAX_TARGET_LIFETIME_MS;
pub const MAX_WAIT_MS: u64 = 30_000;
/// How long an exact stdin retry waits for the identical in-flight write to settle.
pub const MAX_STDIN_REPLAY_WAIT_MS: u64 = 2_000;
pub const MAX_TARGET_LIFETIME_MS: u64 = 8 * 60 * 60 * 1_000;
/// A physical generation refuses unbounded binding preparation. Managed bindings receive a
/// distinct kernel uid from this range; the ordinary additional-sandbox shell remains
/// `ENVIRONMENT_TOOL_UID` and all Tool processes share `ENVIRONMENT_TOOL_GID` for workspace collaboration.
pub const MAX_PREPARED_BINDINGS: usize = 4_096;
pub const MANAGED_BINDING_UID_MIN: u32 = 65_536;
pub const MANAGED_BINDING_UID_SPAN: u32 = 2_000_000_000 - MANAGED_BINDING_UID_MIN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolIdentity {
    pub uid: u32,
    pub gid: u32,
    pub supervisor_uid: u32,
}

/// Whether the kernel Tool boundary is enforced. Production always enforces; the plain test
/// harness runs unenforced. The half-configured combinations (an identity without a boundary
/// library, or the reverse) are unrepresentable, so validation cannot be skipped by accident.
#[derive(Debug, Clone)]
pub enum Sandboxing {
    Enforced {
        identity: ToolIdentity,
        boundary_library: PathBuf,
    },
    Unenforced,
}

impl Sandboxing {
    #[must_use]
    pub fn identity(&self) -> Option<ToolIdentity> {
        match self {
            Self::Enforced { identity, .. } => Some(*identity),
            Self::Unenforced => None,
        }
    }

    #[must_use]
    pub fn boundary_library(&self) -> Option<&Path> {
        match self {
            Self::Enforced {
                boundary_library, ..
            } => Some(boundary_library),
            Self::Unenforced => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub workspace: PathBuf,
    pub state_dir: PathBuf,
    pub tool_dir: PathBuf,
    pub object_dir: PathBuf,
    pub tool_runner: PathBuf,
    pub sandboxing: Sandboxing,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let listen = std::env::var(ENV_LISTEN)
            .unwrap_or_else(|_| "0.0.0.0:8080".into())
            .parse()
            .map_err(|error| anyhow::anyhow!("{ENV_LISTEN}: {error}"))?;
        let workspace =
            PathBuf::from(std::env::var(ENV_WORKSPACE).unwrap_or_else(|_| "/workspace".into()));
        let state_dir = PathBuf::from(
            std::env::var(ENV_STATE_DIR).unwrap_or_else(|_| "/var/environment".into()),
        );
        let tool_dir = PathBuf::from(
            std::env::var(ENV_TOOL_DIR).unwrap_or_else(|_| "/var/environment/tools".into()),
        );
        let tool_runner = PathBuf::from(
            std::env::var(ENV_TOOL_RUNNER)
                .unwrap_or_else(|_| "/usr/local/lib/environment/tool-runner.mjs".into()),
        );
        let tool_boundary_library = PathBuf::from(
            std::env::var(ENV_TOOL_BOUNDARY_LIBRARY)
                .unwrap_or_else(|_| "/usr/local/lib/environment/tool-boundary.so".into()),
        );
        let parse_id = |name: &'static str| -> anyhow::Result<u32> {
            std::env::var(name)
                .map_err(|_| {
                    anyhow::anyhow!("{name} is required in the production Environment image")
                })?
                .parse()
                .map_err(|error| anyhow::anyhow!("{name}: {error}"))
        };
        let config = Self {
            listen,
            workspace,
            object_dir: state_dir.join("objects"),
            state_dir,
            tool_dir,
            tool_runner,
            sandboxing: Sandboxing::Enforced {
                identity: ToolIdentity {
                    supervisor_uid: parse_id(ENV_SUPERVISOR_UID)?,
                    uid: parse_id(ENV_TOOL_UID)?,
                    gid: parse_id(ENV_TOOL_GID)?,
                },
                boundary_library: tool_boundary_library,
            },
        };
        config.validate()?;
        Ok(config)
    }

    pub fn for_test(root: &Path) -> Self {
        Self {
            listen: "127.0.0.1:0".parse().expect("test listener"),
            workspace: root.join("workspace"),
            state_dir: root.join("state"),
            tool_dir: root.join("state/tools"),
            object_dir: root.join("state/objects"),
            tool_runner: PathBuf::from("image/tool-runner.mjs"),
            sandboxing: Sandboxing::Unenforced,
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        let Sandboxing::Enforced {
            identity,
            boundary_library,
        } = &self.sandboxing
        else {
            return Ok(());
        };
        anyhow::ensure!(
            identity.supervisor_uid != identity.uid,
            "Environment supervisor and Tool uid must differ"
        );
        anyhow::ensure!(identity.uid != 0, "untrusted Tool uid must not be root");
        anyhow::ensure!(
            identity.uid < MANAGED_BINDING_UID_MIN
                && identity.supervisor_uid < MANAGED_BINDING_UID_MIN,
            "configured supervisor and sandbox uids overlap the managed-binding uid range"
        );
        anyhow::ensure!(
            boundary_library.is_file(),
            "Tool boundary library is unavailable: {}",
            boundary_library.display()
        );
        #[cfg(unix)]
        {
            // SAFETY: this only reads the process credential.
            let current = unsafe { libc::geteuid() };
            anyhow::ensure!(
                current == identity.supervisor_uid,
                "Environment supervisor runs as uid {current}, expected {}",
                identity.supervisor_uid
            );
        }
        Ok(())
    }
}

/// Wall-clock epoch milliseconds. A pre-epoch system clock would silently disable target
/// expiry and skew deadline math if clamped to zero, so it fails loudly instead.
pub fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the system clock is before the Unix epoch")
        .as_millis() as u64
}
