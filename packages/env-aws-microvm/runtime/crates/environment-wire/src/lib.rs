//! Private production-Environment transport framing.
//!
//! Brain owns every public request and response carried here. These envelopes add only request
//! multiplexing and target bootstrap/install commands; they are not a second public protocol.
//! Every connection and run payload pins Brain's exact canonical contract digest.

use brain::environment::{
    SandboxFileContent, SandboxFileList, SandboxFileListRequest, SandboxSearchRequest,
};
use brain_protocol::environment::{
    AcknowledgeTerminalRequest, Acknowledgement, BundleDescriptor, CancelRequest,
    CancellationReceipt, FileEntry, NetworkCeiling, ObjectReference, ObserveRequest,
    OperationObservation, ResourceCeiling, SandboxCopyResult, SandboxExecutionRequest,
    SandboxFileRequest, SandboxFileWriteResult, SandboxTarget, SealedBinding, SubmitReceipt,
    SubmitRequest, WriteStdinReceipt, WriteStdinRequest,
};
use environment_policy::connector::ConnectorClass;
use environment_policy::secret::ControlToken;
use serde::{Deserialize, Serialize};

/// Inline file reads/writes are capped at 1 MiB decoded. A 2 MiB frame admits their padded base64
/// plus the exact request/response envelope without making the WebSocket allocation unbounded.
pub const MAX_INLINE_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_WIRE_FRAME_BYTES: usize = 2 * 1024 * 1024;
/// Private guest transport headroom. Brain's canonical per-Tool bundle limit is narrower, while
/// this matches the aggregate session-bundle ceiling so the transport cannot become the limiter.
pub const MAX_BUNDLE_INSTALL_BYTES: usize = brain_protocol::MAX_SESSION_BUNDLE_BYTES;
pub const MAX_INSTALL_METADATA_BYTES: usize = 64 * 1024;
pub const MAX_INSTALL_BODY_BYTES: usize = MAX_BUNDLE_INSTALL_BYTES + MAX_INSTALL_METADATA_BYTES + 1;
pub use environment_policy::MAX_OBJECT_BYTES;
pub const OBJECT_METADATA_HEADER: &str = "x-aex-object-metadata";
pub const FILE_ENTRY_HEADER: &str = "x-aex-file-entry";
pub const CONTROL_AUTH_HEADER: &str = "x-aex-environment-control";

/// The provider run-hook payload. It contains no cloud credential. Its generation-scoped control
/// bearer authenticates the trusted host after the provider proxy and must never enter logs,
/// formatting, argv, Tool environments, or public Brain projections.
///
/// An allowlist capability is intentionally visible to the sandbox generation. It is a sealed
/// destination grant, not an Aex credential, and must still never appear in logs or process argv.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunPayload {
    pub contract_digest: String,
    pub generation: String,
    pub expires_at_ms: u64,
    pub root_id: String,
    pub owner_session_id: String,
    pub connector: ConnectorClass,
    pub resource_class: String,
    pub resources: ResourceCeiling,
    pub network: NetworkCeiling,
    pub control_token: ControlToken,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowlist_proxy: Option<AllowlistProxy>,
    /// Internal image-promotion canary only. Production Environments always send `None`. When present,
    /// the guest commits the matching terminal response to its private state, writes that response
    /// to the provider socket, and then aborts so release automation can prove the provider never
    /// rearms the same physical generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canary_exit_after_operation_id: Option<String>,
}

/// Deliberately omits `Debug`: `capability` is a bearer grant.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AllowlistProxy {
    pub authority: String,
    pub capability: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunEnvelope {
    #[serde(default, rename = "microvmId")]
    pub microvm_id: Option<String>,
    #[serde(rename = "runHookPayload")]
    pub run_hook_payload: String,
}

// Deliberately no `Debug`: nested fetch/transfer authorities contain presigned credentials.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequestFrame {
    pub request_id: String,
    pub contract_digest: String,
    pub call: RequestCall,
}

// Deliberately no `Debug`: some calls carry one-purpose object authorities.
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RequestCall {
    Submit(Box<SubmitRequest>),
    Observe(ObserveRequest),
    Cancel(CancelRequest),
    AcknowledgeTerminal(AcknowledgeTerminalRequest),
    Status,
    ListFiles(SandboxFileListRequest),
    StatFile(SandboxFileRequest),
    ReadFile(SandboxFileRequest),
    WriteFile(GuestFileWriteRequest),
    ReserveFileEffect(FileEffectIdentity),
    ClaimFileEffect(FileEffectIdentity),
    CompleteFileEffect(FileEffectStoredResult),
    FindFiles(SandboxSearchRequest),
    GrepFiles(SandboxSearchRequest),
    ExecuteSandbox(SandboxExecutionRequest),
    WriteStdin(WriteStdinRequest),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseFrame {
    pub request_id: String,
    pub result: Result<ResponseReply, brain_protocol::environment::EnvironmentError>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "method", content = "result", rename_all = "snake_case")]
pub enum ResponseReply {
    Submit(SubmitReceipt),
    Observe(OperationObservation),
    Cancel(CancellationReceipt),
    AcknowledgeTerminal(Acknowledgement),
    Status(TargetRuntimeStatus),
    ListFiles(SandboxFileList),
    StatFile(FileEntry),
    ReadFile(SandboxFileContent),
    WriteFile(FileEffectStoredResult),
    ReserveFileEffect(FileEffectReservation),
    ClaimFileEffect(FileEffectReservation),
    CompleteFileEffect(FileEffectStoredResult),
    FindFiles(SandboxFileList),
    GrepFiles(SandboxFileList),
    ExecuteSandbox(SubmitReceipt),
    WriteStdin(WriteStdinReceipt),
}

impl RequestCall {
    /// The wire method tag. Lets an ingress pair a reply to its request until the protocol gains
    /// typed request/reply association (tracked on the Brain rewrite backlog).
    #[must_use]
    pub fn method(&self) -> &'static str {
        match self {
            Self::Submit(_) => "submit",
            Self::Observe(_) => "observe",
            Self::Cancel(_) => "cancel",
            Self::AcknowledgeTerminal(_) => "acknowledge_terminal",
            Self::Status => "status",
            Self::ListFiles(_) => "list_files",
            Self::StatFile(_) => "stat_file",
            Self::ReadFile(_) => "read_file",
            Self::WriteFile(_) => "write_file",
            Self::ReserveFileEffect(_) => "reserve_file_effect",
            Self::ClaimFileEffect(_) => "claim_file_effect",
            Self::CompleteFileEffect(_) => "complete_file_effect",
            Self::FindFiles(_) => "find_files",
            Self::GrepFiles(_) => "grep_files",
            Self::ExecuteSandbox(_) => "execute_sandbox",
            Self::WriteStdin(_) => "write_stdin",
        }
    }
}

impl ResponseReply {
    /// The wire method tag carried by this reply; must equal the request's [`RequestCall::method`].
    #[must_use]
    pub fn method(&self) -> &'static str {
        match self {
            Self::Submit(_) => "submit",
            Self::Observe(_) => "observe",
            Self::Cancel(_) => "cancel",
            Self::AcknowledgeTerminal(_) => "acknowledge_terminal",
            Self::Status(_) => "status",
            Self::ListFiles(_) => "list_files",
            Self::StatFile(_) => "stat_file",
            Self::ReadFile(_) => "read_file",
            Self::WriteFile(_) => "write_file",
            Self::ReserveFileEffect(_) => "reserve_file_effect",
            Self::ClaimFileEffect(_) => "claim_file_effect",
            Self::CompleteFileEffect(_) => "complete_file_effect",
            Self::FindFiles(_) => "find_files",
            Self::GrepFiles(_) => "grep_files",
            Self::ExecuteSandbox(_) => "execute_sandbox",
            Self::WriteStdin(_) => "write_stdin",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetRuntimeStatus {
    pub target_ref: String,
    pub generation: String,
    pub root_id: String,
    pub owner_session_id: String,
    pub connector: ConnectorClass,
    pub resource_class: String,
    pub armed: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallBindingRequest {
    pub binding_ref: String,
    pub binding: SealedBinding,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallBundleMetadata {
    pub descriptor: BundleDescriptor,
    pub layer_digest: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallObjectMetadata {
    pub object: ObjectReference,
}

/// Only the trusted adapter serializes and the guest HTTP handler deserializes this bounded body.
/// It deliberately has no `Debug`, keeping values out of normal framing and tracing.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallSecretsRequest {
    pub session_id: String,
    pub generation: String,
    /// Immutable preparation-time union. Values must match it exactly; each binding receives only
    /// its descriptor-declared subset when the Tool child is spawned.
    pub env_names: Vec<String>,
    pub values: std::collections::HashMap<String, String>,
}

/// Private trusted-adapter projection of a canonical file write. A canonical object source also
/// carries a one-purpose storage GET authority; the hosted Environment consumes that authority while
/// staging bytes and must never forward it into the hostile MicroVM.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuestFileWriteRequest {
    pub effect: FileEffectIdentity,
    pub expected_generation: String,
    pub overwrite: bool,
    pub path: String,
    pub source: GuestFileWriteSource,
    pub target: SandboxTarget,
}

/// Closed identity for an effectful file/storage operation after storage authorities have been
/// consumed by the trusted adapter. It contains no object URL, header, or secret.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileEffectIdentity {
    pub kind: FileEffectKind,
    pub operation_id: String,
    pub request_digest: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileEffectKind {
    Write,
    CopyImport,
    CopyExport,
}

/// The only retained file-effect values. Canonical transfer authorities are deliberately absent.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", content = "result", rename_all = "snake_case")]
pub enum FileEffectStoredResult {
    Write(SandboxFileWriteResult),
    Copy(SandboxCopyResult),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "state", content = "result", rename_all = "snake_case")]
pub enum FileEffectReservation {
    New,
    Replay(Box<FileEffectStoredResult>),
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum GuestFileWriteSource {
    #[serde(rename = "inline")]
    Inline { content_base64: String },
    #[serde(rename = "installed_object")]
    InstalledObject { object: ObjectReference },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallReceipt {
    pub installed: bool,
    pub replayed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_mebibyte_inline_file_and_envelope_fit_the_wire_frame() {
        let padded_base64_bytes = MAX_INLINE_FILE_BYTES.div_ceil(3) * 4;
        assert!(padded_base64_bytes + 16 * 1024 < MAX_WIRE_FRAME_BYTES);
    }
}
