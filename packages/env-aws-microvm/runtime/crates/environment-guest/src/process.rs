//! Linux process boundary for untrusted Node22 Tool bundles.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::fd::AsRawFd as _;
#[cfg(unix)]
use std::os::unix::net::UnixStream as StdUnixStream;

use environment_wire::terminal_inline_bytes;
use environment_wire::{BundleDescriptor, OperationEnvelope, TerminalOutcome};
#[cfg(unix)]
use serde::Deserialize;
use tokio::io::AsyncReadExt as _;
use tokio::io::AsyncWriteExt as _;
use tokio::process::ChildStdin;
use tokio::process::Command;
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroize as _;

use crate::config::ToolIdentity;

#[cfg(unix)]
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
#[cfg(unix)]
const MAX_TOOL_RUNNER_REQUEST_BYTES: usize = 256 * 1024;
#[cfg(unix)]
const RESULT_ENVELOPE_HEADROOM_BYTES: u64 = 4096;
#[cfg(unix)]
const RESULT_FD: libc::c_int = 3;
const POST_CHILD_IO_TIMEOUT: Duration = Duration::from_secs(1);

pub struct BundleExecution {
    pub bundle_path: PathBuf,
    pub execute_digest: String,
    pub node_path: PathBuf,
    pub descriptor: BundleDescriptor,
    pub envelope: OperationEnvelope,
    pub workspace: PathBuf,
    pub runner: PathBuf,
    pub environment: HashMap<String, String>,
    pub proxy_environment: HashMap<String, String>,
    pub identity: Option<ToolIdentity>,
    pub boundary_library: Option<PathBuf>,
    pub target_expires_at_ms: u64,
    pub cancellation: CancellationToken,
}

impl Drop for BundleExecution {
    fn drop(&mut self) {
        zeroize_environment(&mut self.environment);
        zeroize_environment(&mut self.proxy_environment);
    }
}

pub struct ExecutionResult {
    pub outcome: TerminalOutcome,
    pub inline: serde_json::Value,
    pub is_error: bool,
    pub exit_code: Option<i64>,
    pub duration_ms: u64,
}

pub struct ShellExecution {
    pub command: String,
    pub cwd: std::fs::File,
    pub workspace: PathBuf,
    pub timeout_ms: u64,
    pub max_output_bytes: u64,
    pub interactive: bool,
    pub proxy_environment: HashMap<String, String>,
    pub identity: Option<ToolIdentity>,
    pub boundary_library: Option<PathBuf>,
    pub target_expires_at_ms: u64,
    pub cancellation: CancellationToken,
    pub control: Option<std::sync::Arc<InteractiveControl>>,
}

impl Drop for ShellExecution {
    fn drop(&mut self) {
        zeroize_environment(&mut self.proxy_environment);
    }
}

fn zeroize_environment(environment: &mut HashMap<String, String>) {
    for value in environment.values_mut() {
        value.zeroize();
    }
    environment.clear();
}

#[derive(Default)]
pub struct InteractiveControl {
    stdin: Mutex<Option<ChildStdin>>,
    ready: Notify,
}

impl InteractiveControl {
    /// Performs one PIPE_BUF-bounded append and optionally closes stdin under the same
    /// per-execution lock. Returning `true` means every requested effect was applied. The caller
    /// retains the idempotency record before entering this method, so an exact retry never writes
    /// or closes the pipe twice after a lost response.
    pub async fn send_atomic(&self, bytes: &[u8], eof: bool) -> bool {
        if bytes.len() > environment_wire::MAX_WRITE_STDIN_BYTES {
            return false;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        loop {
            // Register before checking the slot so child spawn cannot notify between the check and
            // the wait. The per-execution lock, unlike the idempotency book, may be held while this
            // one pipe waits for capacity.
            let notified = self.ready.notified();
            let mut stdin = match tokio::time::timeout_at(deadline, self.stdin.lock()).await {
                Ok(stdin) => stdin,
                Err(_) => return false,
            };
            if let Some(open_stdin) = stdin.as_mut() {
                if !bytes.is_empty()
                    && !matches!(
                        tokio::time::timeout_at(deadline, open_stdin.write(bytes)).await,
                        Ok(Ok(written)) if written == bytes.len()
                    )
                {
                    return false;
                }
                if eof {
                    stdin.take();
                    self.ready.notify_waiters();
                }
                return true;
            }
            drop(stdin);
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return false;
            }
        }
    }

    async fn install(&self, stdin: Option<ChildStdin>) {
        *self.stdin.lock().await = stdin;
        self.ready.notify_waiters();
    }

    async fn close(&self) {
        self.stdin.lock().await.take();
        self.ready.notify_waiters();
    }
}

#[cfg(unix)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunnerResult {
    ok: bool,
    #[serde(default)]
    output: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
}

pub async fn execute_bundle(request: BundleExecution) -> ExecutionResult {
    run_execution("Tool execution", execute_bundle_inner(&request)).await
}

/// One terminal projection for every executor: timing, cancellation, deadline, and failure map
/// identically whether the child was a Tool runner or a sandbox shell.
async fn run_execution<F>(subject: &str, run: F) -> ExecutionResult
where
    F: std::future::Future<Output = Result<ExecutionResult, Failure>>,
{
    let started = Instant::now();
    match run.await {
        Ok(mut result) => {
            result.duration_ms = started.elapsed().as_millis() as u64;
            result
        }
        Err(Failure::Cancelled) => failure(
            TerminalOutcome::Cancelled,
            &format!("{subject} was cancelled"),
            started,
            None,
        ),
        Err(Failure::Deadline) => failure(
            TerminalOutcome::DeadlineExceeded,
            &format!("{subject} exceeded its deadline"),
            started,
            None,
        ),
        Err(Failure::Message { message, exit_code }) => {
            failure(TerminalOutcome::Failed, &message, started, exit_code)
        }
    }
}

#[cfg(unix)]
async fn execute_bundle_inner(request: &BundleExecution) -> Result<ExecutionResult, Failure> {
    if request.envelope.input.kind != serde_json::Value::String("inline".into()) {
        return Err(Failure::message("managed Tool input kind must be inline"));
    }
    let canonical_input = serde_jcs::to_vec(&request.envelope.input)
        .map_err(|_| Failure::message("managed Tool input cannot be canonicalized"))?;
    if canonical_input.len() > environment_wire::MAX_MANAGED_TOOL_INPUT_BYTES {
        return Err(Failure::message(format!(
            "managed Tool input exceeds the {}-byte canonical bound",
            environment_wire::MAX_MANAGED_TOOL_INPUT_BYTES
        )));
    }
    let input = request.envelope.input.value.clone();
    let deadline_at_ms = execution_deadline_at(
        wall_ms(),
        request
            .envelope
            .deadline_at_ms
            .get()
            .min(request.target_expires_at_ms),
        request.envelope.resources.timeout_ms.get(),
    )?;
    let operation_id = request.envelope.operation_id.as_str();
    let description = request
        .descriptor
        .description
        .as_ref()
        .map(|value| value.as_str());
    let body = serde_json::json!({
        "operation_id": operation_id,
        "session_id": request.envelope.session_id.as_str(),
        "seal": {
            "name": request.descriptor.tool_name.as_str(),
            "description": description,
            "contract_digest": request.descriptor.contract_digest.as_str(),
            "execute_digest": request.execute_digest,
            "required_env": request.descriptor.required_env.iter().map(|name| name.as_str()).collect::<Vec<_>>(),
        },
        "input": input,
        "phase": request.envelope.phase,
        "workspace": "/workspace",
        "deadline_ms": request.envelope.deadline_at_ms.get(),
        "max_output_bytes": request.envelope.resources.max_output_bytes.get(),
    });
    let request_bytes = serde_json::to_vec(&body)
        .map_err(|error| Failure::message(format!("could not encode Tool request: {error}")))?;
    if request_bytes.len() > MAX_TOOL_RUNNER_REQUEST_BYTES {
        return Err(Failure::message(format!(
            "Tool runner request exceeds the {MAX_TOOL_RUNNER_REQUEST_BYTES}-byte transport bound"
        )));
    }
    let result_limit = request
        .envelope
        .resources
        .max_output_bytes
        .get()
        .saturating_add(RESULT_ENVELOPE_HEADROOM_BYTES);
    let (result_reader, result_writer) = StdUnixStream::pair().map_err(|error| {
        Failure::message(format!("could not create Tool result channel: {error}"))
    })?;
    result_reader
        .set_nonblocking(true)
        .map_err(|error| Failure::message(format!("could not arm Tool result channel: {error}")))?;
    let result_reader = tokio::net::UnixStream::from_std(result_reader).map_err(|error| {
        Failure::message(format!("could not attach Tool result channel: {error}"))
    })?;
    let result_fd = result_writer.as_raw_fd();

    let mut command = Command::new(&request.node_path);
    configure_node_invocation(&mut command, &request.runner, &request.bundle_path);
    command
        .current_dir(&request.workspace)
        .env_clear()
        .envs(base_environment(&request.workspace))
        .envs(&request.environment)
        // Connector authority is supervisor-owned and cannot be shadowed by a customer secret.
        .envs(&request.proxy_environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    install_boundary_environment(&mut command, request.boundary_library.as_deref());
    install_child_boundary(&mut command, request.identity, None, Some(result_fd));
    let mut child = command
        .spawn()
        .map_err(|error| Failure::message(format!("could not start Node22 Tool: {error}")))?;
    drop(result_writer);
    let process_id = child.id();
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| Failure::message("Tool request channel was not created"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let mut request_task = tokio::spawn(write_runner_request(stdin, request_bytes));
    let mut result_task = tokio::spawn(read_framed_result(result_reader, result_limit));
    let stdout_task = tokio::spawn(read_bounded(stdout, MAX_DIAGNOSTIC_BYTES));
    let stderr_task = tokio::spawn(read_bounded(stderr, MAX_DIAGNOSTIC_BYTES));
    let aborts = [
        request_task.abort_handle(),
        result_task.abort_handle(),
        stdout_task.abort_handle(),
        stderr_task.abort_handle(),
    ];
    let status = tokio::select! {
        status = child.wait() => status.map_err(|error| Failure::message(format!("Tool wait failed: {error}")))?,
        () = request.cancellation.cancelled() => {
            teardown(&mut child, process_id, &aborts, None).await;
            return Err(Failure::Cancelled);
        }
        () = wait_for_wall_deadline(deadline_at_ms) => {
            teardown(&mut child, process_id, &aborts, None).await;
            return Err(Failure::Deadline);
        }
    };
    // A successful/failed leader exit is also an operation boundary. Untrusted descendants may
    // have closed inherited pipes and intentionally remained alive; the seccomp fence keeps them
    // in this group so the supervisor can sweep them before returning a terminal receipt.
    sweep_process_group(process_id).await;
    let request_settled = settle_request_writer(&mut request_task);
    let result_settled = settle_result_reader(&mut result_task);
    let stdout_settled = settle_bounded_reader(stdout_task);
    let stderr_settled = settle_bounded_reader(stderr_task);
    let (_, result_bytes, stdout, stderr) = tokio::join!(
        request_settled,
        result_settled,
        stdout_settled,
        stderr_settled
    );
    // Diagnostics only: the framed fd-3 result is authoritative here, so a failed or torn
    // diagnostic stream deliberately degrades to empty rather than failing the operation.
    let stdout = stdout.and_then(std::io::Result::ok).unwrap_or_default();
    let stderr = stderr.and_then(std::io::Result::ok).unwrap_or_default();
    let exit_code = status.code().map(i64::from);
    let result_bytes = result_bytes.map_err(|_| Failure::Message {
        message: diagnostic("Tool runner produced no result", &stdout, &stderr),
        exit_code,
    })?;
    let result: RunnerResult =
        serde_json::from_slice(&result_bytes).map_err(|error| Failure::Message {
            message: diagnostic(
                &format!("Tool result is invalid: {error}"),
                &stdout,
                &stderr,
            ),
            exit_code,
        })?;
    if result.ok && status.success() {
        let output = result.output.unwrap_or(serde_json::Value::Null);
        enforce_inline_bound(
            &output,
            request.envelope.resources.max_output_bytes.get(),
            "Tool",
        )?;
        return Ok(ExecutionResult {
            outcome: TerminalOutcome::Completed,
            inline: output,
            is_error: false,
            exit_code,
            duration_ms: 0,
        });
    }
    Err(Failure::Message {
        message: diagnostic(
            result.error.as_deref().unwrap_or("Tool execution failed"),
            &stdout,
            &stderr,
        ),
        exit_code,
    })
}

#[cfg(unix)]
fn configure_node_invocation(command: &mut Command, runner: &Path, bundle_path: &Path) {
    command.arg("--use-env-proxy").arg(runner).arg(bundle_path);
}

#[cfg(not(unix))]
async fn execute_bundle_inner(_request: &BundleExecution) -> Result<ExecutionResult, Failure> {
    Err(Failure::message(
        "managed Tool execution requires the Linux Environment guest",
    ))
}

pub async fn execute_shell(request: ShellExecution) -> ExecutionResult {
    run_execution("sandbox execution", execute_shell_inner(&request)).await
}

async fn execute_shell_inner(request: &ShellExecution) -> Result<ExecutionResult, Failure> {
    let deadline_at_ms =
        execution_deadline_at(wall_ms(), request.target_expires_at_ms, request.timeout_ms)?;
    let mut command = Command::new("/bin/bash");
    command
        .arg("-lc")
        .arg(&request.command)
        .env_clear()
        .envs(base_environment(&request.workspace))
        .envs(&request.proxy_environment)
        .stdin(if request.interactive {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    install_boundary_environment(&mut command, request.boundary_library.as_deref());
    install_child_boundary(
        &mut command,
        request.identity,
        Some(
            request.cwd.try_clone().map_err(|error| {
                Failure::message(format!("could not clone sandbox cwd: {error}"))
            })?,
        ),
        None,
    );
    let mut child = command
        .spawn()
        .map_err(|error| Failure::message(format!("could not start sandbox shell: {error}")))?;
    let process_id = child.id();
    if let Some(control) = &request.control {
        control.install(child.stdin.take()).await;
    }
    let output_limit = usize::try_from(request.max_output_bytes)
        .unwrap_or(usize::MAX)
        .min(crate::config::MAX_OPERATION_OUTPUT_BYTES as usize);
    let stdout_task = tokio::spawn(read_bounded(child.stdout.take(), output_limit + 1));
    let stderr_task = tokio::spawn(read_bounded(child.stderr.take(), output_limit + 1));
    let aborts = [stdout_task.abort_handle(), stderr_task.abort_handle()];
    let status = tokio::select! {
        status = child.wait() => status.map_err(|error| Failure::message(format!("sandbox wait failed: {error}")))?,
        () = request.cancellation.cancelled() => {
            teardown(&mut child, process_id, &aborts, request.control.as_deref()).await;
            return Err(Failure::Cancelled);
        }
        () = wait_for_wall_deadline(deadline_at_ms) => {
            teardown(&mut child, process_id, &aborts, request.control.as_deref()).await;
            return Err(Failure::Deadline);
        }
    };
    sweep_process_group(process_id).await;
    if let Some(control) = &request.control {
        control.close().await;
    }
    let (stdout, stderr) = tokio::join!(
        settle_bounded_reader(stdout_task),
        settle_bounded_reader(stderr_task)
    );
    let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
        return Err(Failure::Message {
            message: "sandbox output streams remained open after the command exited".into(),
            exit_code: status.code().map(i64::from),
        });
    };
    let exit_code = status.code().map(i64::from);
    // These bytes are the operation's authoritative output: a pipe error is a failed
    // operation, never a Completed receipt carrying silently truncated output.
    let stdout = stdout.map_err(|error| Failure::Message {
        message: format!("sandbox stdout stream failed: {error}"),
        exit_code,
    })?;
    let stderr = stderr.map_err(|error| Failure::Message {
        message: format!("sandbox stderr stream failed: {error}"),
        exit_code,
    })?;
    if stdout.len().saturating_add(stderr.len()) > output_limit {
        return Err(Failure::Message {
            message: "sandbox output exceeds the sealed output ceiling".into(),
            exit_code,
        });
    }
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    let inline = serde_json::json!({"stdout": stdout, "stderr": stderr});
    enforce_inline_bound(&inline, output_limit as u64, "sandbox")
        .map_err(|failure| failure.with_exit_code(exit_code))?;
    Ok(ExecutionResult {
        outcome: if status.success() {
            TerminalOutcome::Completed
        } else {
            TerminalOutcome::Failed
        },
        inline,
        is_error: !status.success(),
        exit_code,
        duration_ms: 0,
    })
}

fn enforce_inline_bound(
    value: &serde_json::Value,
    sealed_max_bytes: u64,
    executor: &str,
) -> Result<(), Failure> {
    let bytes = terminal_inline_bytes(value)
        .map_err(|_| Failure::message(format!("{executor} result cannot be canonicalized")))?;
    let sealed_max = usize::try_from(sealed_max_bytes).unwrap_or(usize::MAX);
    let effective_max = sealed_max.min(environment_wire::MAX_TOOL_TERMINAL_INLINE_BYTES);
    if bytes > effective_max {
        return Err(Failure::message(format!(
            "{executor} may have completed, but its inline result exceeds the sealed {effective_max}-byte output ceiling; store large data in session storage or the sandbox and return a key/path"
        )));
    }
    Ok(())
}

use crate::config::wall_ms;

fn execution_deadline_at(
    now_ms: u64,
    absolute_deadline_at_ms: u64,
    timeout_ms: u64,
) -> Result<u64, Failure> {
    let deadline_at_ms = absolute_deadline_at_ms.min(now_ms.saturating_add(timeout_ms));
    if deadline_at_ms <= now_ms {
        Err(Failure::Deadline)
    } else {
        Ok(deadline_at_ms)
    }
}

async fn wait_for_wall_deadline(deadline_at_ms: u64) {
    loop {
        let remaining = deadline_at_ms.saturating_sub(wall_ms());
        if remaining == 0 {
            return;
        }
        // A MicroVM snapshot can pause a monotonic timer. Recheck UTC at a short interval so the
        // first poll after auto-resume immediately observes an elapsed sealed wall deadline.
        tokio::time::sleep(Duration::from_millis(remaining.min(1_000))).await;
    }
}

#[cfg(unix)]
async fn write_runner_request(mut stdin: ChildStdin, bytes: Vec<u8>) -> Result<(), Failure> {
    stdin
        .write_all(&bytes)
        .await
        .map_err(|_| Failure::message("Tool request channel closed before the bounded request"))?;
    stdin
        .shutdown()
        .await
        .map_err(|_| Failure::message("Tool request channel could not be closed"))
}

#[cfg(unix)]
async fn read_framed_result(
    mut reader: tokio::net::UnixStream,
    max_bytes: u64,
) -> Result<Vec<u8>, Failure> {
    let mut header = [0u8; 4];
    reader
        .read_exact(&mut header)
        .await
        .map_err(|_| Failure::message("Tool runner produced no complete result frame"))?;
    let declared = u64::from(u32::from_be_bytes(header));
    if declared == 0 || declared > max_bytes {
        return Err(Failure::message(
            "Tool result frame exceeds the sealed output ceiling",
        ));
    }
    let length = usize::try_from(declared)
        .map_err(|_| Failure::message("Tool result frame length is unsupported"))?;
    let mut bytes = vec![0; length];
    reader
        .read_exact(&mut bytes)
        .await
        .map_err(|_| Failure::message("Tool runner produced a truncated result frame"))?;
    Ok(bytes)
}

/// How a post-child I/O task ended inside the bounded settle window.
enum Settled<T> {
    Done(T),
    /// The task itself died (panic or abort) before producing its value.
    Stopped,
    /// The task did not finish inside the window and was aborted.
    Unsettled,
}

/// Terminates the whole child process group and stops every dependent I/O task; the shell path
/// also closes its interactive stdin control.
async fn teardown(
    child: &mut tokio::process::Child,
    process_id: Option<u32>,
    aborts: &[tokio::task::AbortHandle],
    control: Option<&InteractiveControl>,
) {
    terminate_process_group(child, process_id).await;
    for abort in aborts {
        abort.abort();
    }
    if let Some(control) = control {
        control.close().await;
    }
}

/// Awaits one post-child I/O task for the bounded settle window; a task that will not settle is
/// aborted rather than awaited forever.
async fn settle<T>(task: &mut tokio::task::JoinHandle<T>) -> Settled<T> {
    match tokio::time::timeout(POST_CHILD_IO_TIMEOUT, &mut *task).await {
        Ok(Ok(value)) => Settled::Done(value),
        Ok(Err(_)) => Settled::Stopped,
        Err(_) => {
            task.abort();
            Settled::Unsettled
        }
    }
}

#[cfg(unix)]
async fn settle_request_writer(
    task: &mut tokio::task::JoinHandle<Result<(), Failure>>,
) -> Result<(), Failure> {
    match settle(task).await {
        Settled::Done(result) => result,
        Settled::Stopped => Err(Failure::message("Tool request writer stopped unexpectedly")),
        Settled::Unsettled => Err(Failure::message("Tool request writer did not settle")),
    }
}

#[cfg(unix)]
async fn settle_result_reader(
    task: &mut tokio::task::JoinHandle<Result<Vec<u8>, Failure>>,
) -> Result<Vec<u8>, Failure> {
    match settle(task).await {
        Settled::Done(result) => result,
        Settled::Stopped => Err(Failure::message("Tool result reader stopped unexpectedly")),
        Settled::Unsettled => Err(Failure::message(
            "Tool runner produced no complete result frame",
        )),
    }
}

async fn settle_bounded_reader(
    mut task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
) -> Option<std::io::Result<Vec<u8>>> {
    match settle(&mut task).await {
        Settled::Done(bytes) => Some(bytes),
        Settled::Stopped | Settled::Unsettled => None,
    }
}

fn base_environment(workspace: &Path) -> HashMap<String, String> {
    HashMap::from([
        (
            "PATH".into(),
            "/workspace/.environment/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
        ),
        ("HOME".into(), "/home/agent".into()),
        ("USER".into(), "agent".into()),
        ("LOGNAME".into(), "agent".into()),
        ("LANG".into(), "C.UTF-8".into()),
        ("TERM".into(), "dumb".into()),
        ("CI".into(), "1".into()),
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
        (
            "AEX_WORKSPACE".into(),
            workspace.to_string_lossy().into_owned(),
        ),
    ])
}

fn install_boundary_environment(command: &mut Command, library: Option<&Path>) {
    if let Some(library) = library {
        // The dynamic-loader constructor runs after the final bash/Node exec, closing the reset
        // that makes a pre_exec-only PR_SET_DUMPABLE call ineffective. This assignment occurs
        // after all customer and connector environment so it cannot be shadowed.
        command.env("LD_PRELOAD", library);
    }
}

fn install_child_boundary(
    command: &mut Command,
    identity: Option<ToolIdentity>,
    cwd: Option<std::fs::File>,
    result_fd: Option<libc::c_int>,
) {
    #[cfg(unix)]
    // SAFETY: these syscalls run after fork and before exec in the child. They do not allocate.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if let Some(result_fd) = result_fd {
                if result_fd != RESULT_FD && libc::dup2(result_fd, RESULT_FD) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                let flags = libc::fcntl(RESULT_FD, libc::F_GETFD);
                if flags == -1
                    || libc::fcntl(RESULT_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1
                {
                    return Err(std::io::Error::last_os_error());
                }
                if result_fd != RESULT_FD {
                    libc::close(result_fd);
                }
            }
            if let Some(cwd) = &cwd {
                use std::os::fd::AsRawFd as _;
                if libc::fchdir(cwd.as_raw_fd()) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            if let Some(identity) = identity {
                if libc::setgroups(0, std::ptr::null()) == -1
                    || libc::setresgid(identity.gid, identity.gid, identity.gid) == -1
                    || libc::setresuid(identity.uid, identity.uid, identity.uid) == -1
                    || libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) == -1
                {
                    return Err(std::io::Error::last_os_error());
                }
                // The supervisor executable has only CAP_KILL/CAP_SETUID/CAP_SETGID so it can
                // cross the UID boundary and later enforce the child's deadline. Never depend on
                // exec-time capability recalculation: erase every effective/permitted/inheritable
                // and ambient capability in the forked child before untrusted bash/Node code is
                // evaluated. Each managed binding has its own uid; non-dumpability additionally
                // narrows same-binding procfs access for dynamic executables. Static exec resets
                // this flag, so distinct binding uids—not this defense-in-depth flag—are the
                // authoritative cross-secret-subset boundary.
                if clear_child_capabilities() == -1
                    || libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == -1
                    || install_process_group_fence() == -1
                {
                    return Err(std::io::Error::last_os_error());
                }
                libc::umask(0o002);
            }
            Ok(())
        });
    }
    #[cfg(not(unix))]
    let _ = (command, identity, cwd, result_fd);
}

#[cfg(target_os = "linux")]
unsafe fn clear_child_capabilities() -> libc::c_int {
    #[repr(C)]
    struct CapabilityHeader {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CapabilityData {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }

    const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;
    let mut header = CapabilityHeader {
        version: LINUX_CAPABILITY_VERSION_3,
        pid: 0,
    };
    let data = [CapabilityData {
        effective: 0,
        permitted: 0,
        inheritable: 0,
    }; 2];
    // SAFETY: both pointers address fixed-size C-layout values that remain alive for the syscall.
    let cleared = unsafe { libc::syscall(libc::SYS_capset, &mut header, data.as_ptr()) };
    if cleared == -1 {
        return -1;
    }
    // SAFETY: PR_CAP_AMBIENT_CLEAR_ALL has no pointer arguments.
    unsafe {
        libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL,
            0,
            0,
            0,
        )
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
unsafe fn clear_child_capabilities() -> libc::c_int {
    // Environments supports Linux guests only; retain buildability for Unix development hosts.
    0
}

/// Keeps every untrusted descendant in the process group created by the supervisor. Without this
/// inherited seccomp fence, a grandchild can call `setsid`/`setpgid`, leave that group, and outlive
/// an operation deadline. The filter is installed only after the trusted child setup has called
/// `setsid`; `no_new_privs` makes it irreversible for the Tool and all descendants.
#[cfg(target_os = "linux")]
unsafe fn install_process_group_fence() -> libc::c_int {
    const BPF_LD_W_ABS: u16 = 0x20;
    const BPF_JMP_JEQ_K: u16 = 0x15;
    const BPF_RET_K: u16 = 0x06;
    const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
    const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
    const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
    const SECCOMP_MODE_FILTER: libc::c_ulong = 2;
    const SECCOMP_DATA_NR_OFFSET: u32 = 0;
    const SECCOMP_DATA_ARCH_OFFSET: u32 = 4;
    #[cfg(target_arch = "x86_64")]
    const AUDIT_ARCH: u32 = 0xc000_003e;
    #[cfg(target_arch = "aarch64")]
    const AUDIT_ARCH: u32 = 0xc000_00b7;
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    compile_error!("the Environment guest seccomp fence supports only x86_64 and aarch64 Linux");

    const fn statement(code: u16, value: u32) -> libc::sock_filter {
        libc::sock_filter {
            code,
            jt: 0,
            jf: 0,
            k: value,
        }
    }

    const fn jump(code: u16, value: u32, yes: u8, no: u8) -> libc::sock_filter {
        libc::sock_filter {
            code,
            jt: yes,
            jf: no,
            k: value,
        }
    }

    let mut filter = [
        statement(BPF_LD_W_ABS, SECCOMP_DATA_ARCH_OFFSET),
        jump(BPF_JMP_JEQ_K, AUDIT_ARCH, 1, 0),
        statement(BPF_RET_K, SECCOMP_RET_KILL_PROCESS),
        statement(BPF_LD_W_ABS, SECCOMP_DATA_NR_OFFSET),
        jump(BPF_JMP_JEQ_K, libc::SYS_setsid as u32, 0, 1),
        statement(BPF_RET_K, SECCOMP_RET_ERRNO | libc::EPERM as u32),
        jump(BPF_JMP_JEQ_K, libc::SYS_setpgid as u32, 0, 1),
        statement(BPF_RET_K, SECCOMP_RET_ERRNO | libc::EPERM as u32),
        statement(BPF_RET_K, SECCOMP_RET_ALLOW),
    ];
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };
    // SAFETY: `program` and its fixed filter array remain alive for the syscall. The kernel copies
    // the program before returning, and PR_SET_NO_NEW_PRIVS was set immediately beforehand.
    unsafe {
        libc::prctl(
            libc::PR_SET_SECCOMP,
            SECCOMP_MODE_FILTER,
            &program as *const libc::sock_fprog,
        )
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
unsafe fn install_process_group_fence() -> libc::c_int {
    // Environments supports Linux guests only; retain buildability for Unix development hosts.
    0
}

async fn terminate_process_group(child: &mut tokio::process::Child, process_id: Option<u32>) {
    #[cfg(unix)]
    if let Some(process_id) = process_id {
        // SAFETY: the child created its own process group with setsid.
        unsafe { libc::kill(-(process_id as i32), libc::SIGTERM) };
        let leader_reaped = matches!(
            tokio::time::timeout(Duration::from_secs(2), child.wait()).await,
            Ok(Ok(_))
        );
        // Always sweep the group even if its leader exited after SIGTERM. A hostile descendant
        // can ignore TERM and outlive a cooperative shell leader while retaining the same pgid.
        // SAFETY: same isolated child process group.
        unsafe { libc::kill(-(process_id as i32), libc::SIGKILL) };
        if !leader_reaped {
            let _ = child.wait().await;
        }
        reap_process_group(process_id).await;
        return;
    }
    let _ = process_id;
    let _ = child.kill().await;
}

async fn sweep_process_group(process_id: Option<u32>) {
    #[cfg(unix)]
    if let Some(process_id) = process_id {
        // SAFETY: the child created this isolated process group with setsid. The leader was just
        // reaped, and the seccomp fence prevents any descendant from leaving or changing groups.
        unsafe { libc::kill(-(process_id as i32), libc::SIGKILL) };
        reap_process_group(process_id).await;
    }
    #[cfg(not(unix))]
    let _ = process_id;
}

#[cfg(unix)]
async fn reap_process_group(process_id: u32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    loop {
        let mut status = 0;
        // SAFETY: this waits only for descendants in the isolated operation process group. The
        // leader has already been reaped through Tokio, so it cannot steal that child status.
        let waited = unsafe { libc::waitpid(-(process_id as i32), &mut status, libc::WNOHANG) };
        if waited > 0 {
            continue;
        }
        if waited == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn read_bounded<R>(reader: Option<R>, limit: usize) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(reader) = reader else {
        return Ok(Vec::new());
    };
    let mut bytes = Vec::new();
    // A mid-stream read error must surface: in the shell path these bytes ARE the
    // authoritative result, and partial output reported as Completed is a silent lie.
    reader.take(limit as u64).read_to_end(&mut bytes).await?;
    Ok(bytes)
}

#[cfg(unix)]
fn diagnostic(prefix: &str, stdout: &[u8], stderr: &[u8]) -> String {
    let mut message = prefix.to_owned();
    for (name, bytes) in [("stderr", stderr), ("stdout", stdout)] {
        if !bytes.is_empty() {
            message.push_str(&format!("\n{name}: {}", String::from_utf8_lossy(bytes)));
        }
    }
    crate::errors::truncate_utf8(&mut message, MAX_DIAGNOSTIC_BYTES);
    message
}

fn failure(
    outcome: TerminalOutcome,
    message: &str,
    started: Instant,
    exit_code: Option<i64>,
) -> ExecutionResult {
    ExecutionResult {
        outcome,
        inline: serde_json::json!({"error": message}),
        is_error: true,
        exit_code,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

enum Failure {
    Cancelled,
    Deadline,
    Message {
        message: String,
        exit_code: Option<i64>,
    },
}

impl Failure {
    fn message(message: impl Into<String>) -> Self {
        Self::Message {
            message: message.into(),
            exit_code: None,
        }
    }

    fn with_exit_code(self, exit_code: Option<i64>) -> Self {
        match self {
            Self::Message { message, .. } => Self::Message { message, exit_code },
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::time::Duration;

    use super::*;

    #[test]
    fn child_environment_is_an_explicit_secret_free_allowlist() {
        let environment = base_environment(Path::new("/workspace"));
        assert_eq!(
            environment.get("HOME").map(String::as_str),
            Some("/home/agent")
        );
        assert!(!environment.contains_key("npm_config_prefix"));
        assert!(!environment.contains_key("PIP_CACHE_DIR"));
        for inherited_secret in [
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "ENVIRONMENT_CAPABILITY_SIGNING_KEY_ID",
            "ENVIRONMENT_EGRESS_GATEWAY_AUTHORITY",
        ] {
            assert!(!environment.contains_key(inherited_secret));
        }
    }

    #[cfg(unix)]
    #[test]
    fn node_tools_use_the_supervisor_proxy() {
        let mut command = Command::new("node");
        configure_node_invocation(
            &mut command,
            Path::new("/opt/aex/tool-runner.mjs"),
            Path::new("/workspace/tool.mjs"),
        );

        let arguments = command
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            [
                "--use-env-proxy",
                "/opt/aex/tool-runner.mjs",
                "/workspace/tool.mjs"
            ]
        );
    }

    #[test]
    fn canonical_terminal_inline_bound_accepts_exact_and_rejects_plus_one() {
        let limit = environment_wire::MAX_TOOL_TERMINAL_INLINE_BYTES;
        let exact = serde_json::Value::String("x".repeat(limit - 2));
        let too_large = serde_json::Value::String("x".repeat(limit - 1));

        assert_eq!(terminal_inline_bytes(&exact).unwrap(), limit);
        assert!(enforce_inline_bound(&exact, limit as u64, "Tool").is_ok());
        assert_eq!(terminal_inline_bytes(&too_large).unwrap(), limit + 1);
        assert!(enforce_inline_bound(&too_large, (limit + 1) as u64, "Tool").is_err());
    }

    #[test]
    fn relative_timeout_ceiling_cannot_be_widened_by_a_far_absolute_deadline() {
        assert_eq!(execution_deadline_at(1_000, u64::MAX, 25).ok(), Some(1_025));
        assert_eq!(execution_deadline_at(1_000, 1_010, 25).ok(), Some(1_010));
        assert!(execution_deadline_at(1_000, 1_000, 25).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn framed_result_completes_without_waiting_for_pipe_eof() {
        let (reader, mut writer) = tokio::net::UnixStream::pair().unwrap();
        let body = br#"{"ok":true,"output":"complete"}"#;
        writer
            .write_all(&(body.len() as u32).to_be_bytes())
            .await
            .unwrap();
        writer.write_all(body).await.unwrap();

        let result = tokio::time::timeout(
            Duration::from_millis(100),
            read_framed_result(reader, body.len() as u64),
        )
        .await
        .expect("a complete frame must not wait for the still-open writer")
        .unwrap_or_else(|_| panic!("a complete frame must be accepted"));
        assert_eq!(result, body);
        drop(writer);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn framed_result_rejects_an_oversized_declaration_before_body_read() {
        let (reader, mut writer) = tokio::net::UnixStream::pair().unwrap();
        writer.write_all(&1025u32.to_be_bytes()).await.unwrap();
        let result =
            tokio::time::timeout(Duration::from_millis(100), read_framed_result(reader, 1024))
                .await
                .expect("an oversized header must be rejected without reading its body");
        assert!(result.is_err());
    }

    /// The Tool IPC contract exists in two languages: this supervisor and
    /// `image/tool-runner.mjs`. Pin the runner's literals to the Rust constants so a change on
    /// either side fails the build instead of silently producing a mismatched frame protocol.
    #[cfg(unix)]
    #[test]
    fn the_tool_runner_carries_the_supervisor_ipc_contract() {
        let runner = include_str!("../../../image/tool-runner.mjs");
        assert!(
            runner.contains(&format!(
                "maxOutputBytes + {RESULT_ENVELOPE_HEADROOM_BYTES}"
            )),
            "runner envelope headroom must match RESULT_ENVELOPE_HEADROOM_BYTES"
        );
        assert!(
            runner.contains(&format!("writeSync({RESULT_FD}, frame")),
            "runner must write frames to the supervisor result fd"
        );
        assert!(
            runner.contains("writeUInt32BE"),
            "runner must length-prefix frames big-endian, as the supervisor reads them"
        );
    }
}
