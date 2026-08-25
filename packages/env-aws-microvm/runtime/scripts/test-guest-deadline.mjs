import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile, stat } from "node:fs/promises";

const authority = "http://127.0.0.1:8080";
const environmentContractDigest = "7aea6ad07f67b322300c752017bf2c5cda692e0e4fc6579fe6af5a7f7ab606dc";
const controlToken = `control-${"a".repeat(64)}`;

const payload = {
  contract_digest: environmentContractDigest,
  generation: "generation-ci",
  expires_at_ms: Date.now() + 60_000,
  root_id: "root-ci",
  owner_session_id: "session-ci",
  connector: "none",
  resource_class: "microvm-1gb",
  resources: { max_output_bytes: 65536, timeout_ms: 60000 },
  network: { kind: "none" },
  control_token: controlToken,
};
const armed = await fetch(`${authority}/aws/lambda-microvms/runtime/v1/run`, {
  method: "POST",
  headers: { "content-type": "text/plain" },
  body: JSON.stringify({ microvmId: "mvm-ci", runHookPayload: JSON.stringify(payload) }),
});
assert.equal(armed.status, 200, await armed.text());

const deniedRoot = await fetch(`${authority}/`);
assert.equal(deniedRoot.status, 401);
const root = await fetch(`${authority}/`, {
  headers: { "x-aex-environment-control": controlToken },
}).then((response) => response.json());
assert.equal(root.contract_digest, environmentContractDigest);

const socket = new WebSocket(
  "ws://127.0.0.1:8080/",
  [`aex-environment-control.${controlToken}`],
);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});

let nextRequest = 1;
async function call(method, params) {
  const requestId = `deadline-${nextRequest++}`;
  const response = new Promise((resolve, reject) => {
    const timeout = setTimeout(
      () => reject(new Error(`guest ${method} response timed out`)),
      15_000,
    );
    const onMessage = (event) => {
      const frame = JSON.parse(event.data);
      if (frame.request_id !== requestId) return;
      clearTimeout(timeout);
      socket.removeEventListener("message", onMessage);
      if (frame.result.Err !== undefined) {
        reject(new Error(`guest ${method} failed: ${JSON.stringify(frame.result.Err)}`));
      } else {
        resolve(frame.result.Ok.result);
      }
    };
    socket.addEventListener("message", onMessage);
  });
  socket.send(JSON.stringify({
    request_id: requestId,
    contract_digest: environmentContractDigest,
    call: { method, params },
  }));
  return response;
}

const TARGET = {
  binding_ref: "sandbox-binding-ci",
  kind: "additional",
  root_id: "root-ci",
  sandbox_id: "sandbox-ci",
  session_id: "session-ci",
};

// Seals a request in place: the canonical digest is computed over the value without its own
// request_digest (and any listed non-canonical fields), exactly as the guest re-derives it.
function sealed(value, omit = []) {
  const projection = { ...value };
  delete projection.request_digest;
  for (const field of omit) delete projection[field];
  value.request_digest = createHash("sha256")
    .update(canonicalJson(projection))
    .digest("hex");
  return value;
}

function execution(id, prefix, timeoutMs) {
  const value = {
    execution_id: id,
    expected_generation: "generation-ci",
    input: {
      // The shell leader accepts TERM while its descendant ignores it. The supervisor must still
      // sweep the entire process group after reaping the leader, not mistake leader exit for
      // complete cancellation.
      command: `echo $$ > /workspace/${prefix}-parent.pid; setsid bash -c 'echo $$ > /workspace/${prefix}-escaped.pid; trap "" TERM; while :; do sleep 1; done' 2>/workspace/${prefix}-setsid.err || true; bash -c 'trap "" TERM; echo $$ > /workspace/${prefix}-child.pid; while :; do sleep 1; done'`,
      cwd: "/workspace",
      interactive: false,
    },
    network: { kind: "none" },
    request_digest: "0".repeat(64),
    resources: { max_output_bytes: 1024, timeout_ms: timeoutMs },
    target: { ...TARGET },
  };
  return sealed(value);
}

function stdinRequest(executionId, operationId, text, eof = false) {
  const value = {
    eof,
    execution_id: executionId,
    expected_generation: "generation-ci",
    operation_id: operationId,
    request_digest: "0".repeat(64),
    target: { ...TARGET },
    text,
  };
  return sealed(value);
}

function sandboxExecution(executionId, command, timeoutMs, interactive = false) {
  const value = {
    execution_id: executionId,
    expected_generation: "generation-ci",
    input: { command, cwd: "/workspace", interactive },
    network: { kind: "none" },
    request_digest: "0".repeat(64),
    resources: { max_output_bytes: 65536, timeout_ms: timeoutMs },
    target: { ...TARGET },
  };
  return sealed(value);
}

const deadlineExecution = execution("deadline-execution-ci", "deadline", 250);
const receipt = await call("execute_sandbox", deadlineExecution);
const observation = await call("observe", {
  cursor: "0",
  operation: receipt.operation,
  wait_ms: 10_000,
});
assert.equal(observation.state, "terminal");
assert.equal(observation.terminal.outcome, "deadline_exceeded");

for (const name of ["deadline-parent.pid", "deadline-child.pid"]) {
  const pid = Number.parseInt(await readFile(`/workspace/${name}`, "utf8"), 10);
  assert.ok(Number.isSafeInteger(pid) && pid > 1);
  assert.throws(() => process.kill(pid, 0), { code: "ESRCH" });
}
await assert.rejects(stat("/workspace/deadline-escaped.pid"), { code: "ENOENT" });
assert.match(await readFile("/workspace/deadline-setsid.err", "utf8"), /not permitted/iu);

const cancelExecution = execution("cancel-execution-ci", "cancel", 60_000);
const cancelReceipt = await call("execute_sandbox", cancelExecution);
await waitForFile("/workspace/cancel-child.pid");
const cancelled = await call("cancel", {
  operation: cancelReceipt.operation,
  reason: "image conformance cancellation",
});
assert.equal(cancelled.accepted, true);
const cancelObservation = await call("observe", {
  cursor: "0",
  operation: cancelReceipt.operation,
  wait_ms: 10_000,
});
assert.equal(cancelObservation.state, "terminal");
assert.equal(cancelObservation.terminal.outcome, "cancelled");
for (const name of ["cancel-parent.pid", "cancel-child.pid"]) {
  const pid = Number.parseInt(await readFile(`/workspace/${name}`, "utf8"), 10);
  assert.ok(Number.isSafeInteger(pid) && pid > 1);
  assert.throws(() => process.kill(pid, 0), { code: "ESRCH" });
}
await assert.rejects(stat("/workspace/cancel-escaped.pid"), { code: "ENOENT" });
assert.match(await readFile("/workspace/cancel-setsid.err", "utf8"), /not permitted/iu);

// A leader that exits successfully does not grant its background descendants a sandbox daemon
// lifetime. They remain in the fenced process group, which the supervisor kills and reaps before
// returning the terminal observation.
const completedExecution = sandboxExecution(
  "completed-background-execution-ci",
  String.raw`bash -c 'trap "" TERM; echo $$ > /workspace/completed-background-child.pid; while :; do sleep 1; done' >/dev/null 2>&1 & while test ! -s /workspace/completed-background-child.pid; do sleep 0.01; done`,
  10_000,
);
const completedReceipt = await call("execute_sandbox", completedExecution);
const completedObservation = await call("observe", {
  cursor: "0",
  operation: completedReceipt.operation,
  wait_ms: 10_000,
});
assert.equal(completedObservation.state, "terminal");
assert.equal(completedObservation.terminal.outcome, "completed");
const completedChild = Number.parseInt(
  await readFile("/workspace/completed-background-child.pid", "utf8"),
  10,
);
assert.ok(Number.isSafeInteger(completedChild) && completedChild > 1);
assert.throws(() => process.kill(completedChild, 0), { code: "ESRCH" });

// Every managed binding receives a distinct kernel uid while retaining gid1000 and umask 0002 for
// ordinary shared-workspace output. This is the authoritative procfs boundary: the fixture also
// execs a static helper, for which execve resets dumpability and LD_PRELOAD cannot run. A second
// parallel call to the same immutable binding must retain the same uid; a no-secret binding and
// the additional-sandbox uid1000 must not read either process's procfs surfaces.
const secret = "per-binding-secret-that-must-never-cross-bindings";
const contractDigest = "a".repeat(64);
const bundle = Buffer.from(`
import { spawn } from "node:child_process";
import { writeFile } from "node:fs/promises";
export default {
  kind: "brain.tool-runtime",
  name: "proc_secret_fixture",
  description: "Per-binding procfs isolation fixture.",
  contractDigest: ${JSON.stringify(contractDigest)},
  input: {},
  output: {},
  requiredEnv: ["PROC_SECRET"],
  execute: async ({ slot }) => {
    await writeFile(
      \`/workspace/proc-secret-\${slot}.json\`,
      JSON.stringify({ pid: process.pid, uid: process.getuid() }),
      { mode: 0o660 },
    );
    if (slot === "primary") {
      spawn(
        "/usr/local/lib/environment/proc-secret-static",
        ["/workspace/proc-secret-static.pid"],
        { stdio: "ignore" },
      );
    }
    await new Promise((resolve) => setTimeout(resolve, 60_000));
    return { secretLength: process.env.PROC_SECRET.length };
  },
};
`);
// Installs one fixture bundle plus its sealed binding; both installs derive from one descriptor.
async function installFixtureBinding(bundleBytes, options) {
  const digest = createHash("sha256").update(bundleBytes).digest("hex");
  const descriptor = {
    bundle_digest: digest,
    bytes: bundleBytes.length,
    contract_digest: contractDigest,
    description: options.description,
    object: {
      bytes: bundleBytes.length,
      object_id: options.objectId,
      sha256: digest,
    },
    required_env: options.requiredEnv,
    runtime: "node22",
    tool_name: options.toolName,
  };
  await postInstall(
    `/internal/bundles/${digest}`,
    Buffer.concat([Buffer.from(`${JSON.stringify({ descriptor })}\n`), bundleBytes]),
    "application/octet-stream",
  );
  await postInstall("/internal/bindings", {
    binding_ref: options.bindingRef,
    binding: {
      binding_id: options.bindingRef,
      bundle: descriptor,
      capability: options.toolName,
      contract_digest: contractDigest,
      implementation_identity: options.implementationIdentity,
      policy_digest: options.policyDigest,
      realm: "aex_managed",
      realm_id: "aex",
      required_capabilities: ["execution"],
      root_id: "root-ci",
      session_id: "session-ci",
    },
  });
  return descriptor;
}

await installFixtureBinding(bundle, {
  bindingRef: "proc-secret-binding-ci",
  toolName: "proc_secret_fixture",
  description: "Per-binding procfs isolation fixture.",
  objectId: "proc-secret-bundle-ci",
  requiredEnv: ["PROC_SECRET"],
  implementationIdentity: "b".repeat(64),
  policyDigest: "c".repeat(64),
});

const attackerBundle = Buffer.from(`
import { open, readFile, readdir, rename, writeFile } from "node:fs/promises";
async function probe(pid) {
  const result = {};
  try { await readFile(\`/proc/\${pid}/environ\`); result.environ = "readable"; }
  catch { result.environ = "denied"; }
  try { await readdir(\`/proc/\${pid}/fd\`); result.fd = "readable"; }
  catch { result.fd = "denied"; }
  try {
    const handle = await open(\`/proc/\${pid}/mem\`, "r");
    try { await handle.read(Buffer.alloc(1), 0, 1, 0); result.mem = "readable"; }
    finally { await handle.close(); }
  } catch { result.mem = "denied"; }
  return result;
}
async function probeLegacyIpc(operationId, index) {
  const requestPath = \`/var/environment/tool-ipc/\${operationId}.request.json\`;
  const resultPath = \`/var/environment/tool-ipc/\${operationId}.result.json\`;
  const replacement = \`/workspace/ipc-attacker-replacement-\${index}\`;
  const result = {};
  try { await readFile(requestPath); result.read = "readable"; }
  catch { result.read = "denied"; }
  try { await writeFile(resultPath, '{"ok":true,"output":"forged"}'); result.precreate = "writable"; }
  catch { result.precreate = "denied"; }
  await writeFile(replacement, "replacement", { mode: 0o660 });
  try { await rename(replacement, requestPath); result.replace = "writable"; }
  catch { result.replace = "denied"; }
  return result;
}
export default {
  kind: "brain.tool-runtime",
  name: "proc_attacker_fixture",
  description: "Different-binding procfs attacker fixture.",
  contractDigest: ${JSON.stringify(contractDigest)},
  input: {}, output: {}, requiredEnv: [],
  execute: async ({ targets, ipcOperations }) => ({
    uid: process.getuid(),
    controlPort: await fetch("http://127.0.0.1:8080/", {
      signal: AbortSignal.timeout(1000),
    }).then((response) => response.status === 401 ? "denied" : "reachable", () => "denied"),
    probes: Object.fromEntries(await Promise.all(
      Object.entries(targets).map(async ([name, pid]) => [name, await probe(pid)]),
    )),
    ipc: Object.fromEntries(await Promise.all(
      ipcOperations.map(async (operationId, index) => [
        operationId,
        await probeLegacyIpc(operationId, index),
      ]),
    )),
  }),
};
`);
await installFixtureBinding(attackerBundle, {
  bindingRef: "proc-attacker-binding-ci",
  toolName: "proc_attacker_fixture",
  description: "Different-binding procfs attacker fixture.",
  objectId: "proc-attacker-bundle-ci",
  requiredEnv: [],
  implementationIdentity: "d".repeat(64),
  policyDigest: "e".repeat(64),
});
await postInstall("/internal/secrets", {
  env_names: ["PROC_SECRET"],
  generation: "generation-ci",
  session_id: "session-ci",
  values: { PROC_SECRET: secret },
});

const managedEnvelope = (operationId, bindingRef, capability, input) => {
  const envelope = {
    binding_ref: bindingRef,
    caller_id: "caller-ci",
    capability,
    deadline_at_ms: Date.now() + 60_000,
    fence: 1,
    generation: "generation-ci",
    input: { kind: "inline", value: input },
    network: { kind: "none" },
    operation_id: operationId,
    request_digest: "0".repeat(64),
    resources: { max_output_bytes: 65536, timeout_ms: 60_000 },
    root_id: "root-ci",
    session_id: "session-ci",
    target_ref: "mvm-ci",
    trace: {},
    turn_id: "turn-ci",
  };
  return sealed(envelope, ["trace"]);
};
const primaryEnvelope = managedEnvelope(
  "proc-secret-primary-operation-ci",
  "proc-secret-binding-ci",
  "proc_secret_fixture",
  { slot: "primary", ipcSentinel: "primary-private-ipc-input" },
);
const sameBindingEnvelope = managedEnvelope(
  "proc-secret-same-binding-operation-ci",
  "proc-secret-binding-ci",
  "proc_secret_fixture",
  { slot: "same-binding", ipcSentinel: "same-binding-private-ipc-input" },
);
const managedReceipt = await call("submit", {
  envelope: primaryEnvelope,
  wait_up_to_ms: 0,
});
const sameBindingReceipt = await call("submit", {
  envelope: sameBindingEnvelope,
  wait_up_to_ms: 0,
});
await waitForFile("/workspace/proc-secret-primary.json");
await waitForFile("/workspace/proc-secret-same-binding.json");
await waitForFile("/workspace/proc-secret-static.pid");
const primary = JSON.parse(await readFile("/workspace/proc-secret-primary.json", "utf8"));
const sameBinding = JSON.parse(await readFile("/workspace/proc-secret-same-binding.json", "utf8"));
const staticPid = Number.parseInt(await readFile("/workspace/proc-secret-static.pid", "utf8"), 10);
assert.equal(primary.uid, sameBinding.uid);
assert.ok(primary.uid >= 65_536);

const attackerReceipt = await call("submit", {
  envelope: managedEnvelope(
    "proc-attacker-operation-ci",
    "proc-attacker-binding-ci",
    "proc_attacker_fixture",
    {
      targets: { node: primary.pid, static: staticPid },
      ipcOperations: [primaryEnvelope.operation_id, sameBindingEnvelope.operation_id],
    },
  ),
  wait_up_to_ms: 0,
});
const attackerObservation = await call("observe", {
  cursor: "0",
  operation: attackerReceipt.operation,
  wait_ms: 10_000,
});
assert.equal(attackerObservation.state, "terminal");
assert.equal(attackerObservation.terminal.outcome, "completed");
assert.notEqual(attackerObservation.terminal.inline.uid, primary.uid);
assert.equal(attackerObservation.terminal.inline.controlPort, "denied");
assert.equal(JSON.stringify(attackerObservation).includes(secret), false);
for (const target of ["node", "static"]) {
  assert.deepEqual(attackerObservation.terminal.inline.probes[target], {
    environ: "denied",
    fd: "denied",
    mem: "denied",
  });
}
for (const operationId of [primaryEnvelope.operation_id, sameBindingEnvelope.operation_id]) {
  assert.deepEqual(attackerObservation.terminal.inline.ipc[operationId], {
    read: "denied",
    precreate: "denied",
    replace: "denied",
  });
}
assert.equal(
  JSON.stringify(attackerObservation).includes("private-ipc-input"),
  false,
);

const procProbe = sandboxExecution(
  "proc-secret-probe-ci",
  String.raw`
set -u
echo self_uid=$(id -u)
file /usr/local/lib/environment/proc-secret-static
for target in node:${primary.pid} static:${staticPid}; do
  name=$(printf '%s' "$target" | cut -d: -f1)
  pid=$(printf '%s' "$target" | cut -d: -f2)
  if cat "/proc/$pid/environ" >/dev/null 2>&1; then echo "$name-environ=readable"; else echo "$name-environ=denied"; fi
  if ls "/proc/$pid/fd" >/dev/null 2>&1; then echo "$name-fd=readable"; else echo "$name-fd=denied"; fi
  if dd if="/proc/$pid/mem" of=/dev/null bs=1 count=1 status=none 2>/dev/null; then echo "$name-mem=readable"; else echo "$name-mem=denied"; fi
done
awk '/^(CapInh|CapPrm|CapEff|CapAmb|NoNewPrivs):/{print}' /proc/self/status
if test -e /var/environment/tool-ipc; then echo shared-ipc=present; else echo shared-ipc=absent; fi
`,
  10_000,
);
const probeReceipt = await call("execute_sandbox", procProbe);
const probeObservation = await call("observe", {
  cursor: "0",
  operation: probeReceipt.operation,
  wait_ms: 10_000,
});
assert.equal(probeObservation.state, "terminal");
assert.equal(probeObservation.terminal.outcome, "completed");
const probeOutput = probeObservation.terminal.inline.stdout;
assert.equal(probeOutput.includes(secret), false);
assert.match(probeOutput, /^self_uid=1000$/mu);
assert.match(probeOutput, /proc-secret-static:.*statically linked/iu);
for (const target of ["node", "static"]) {
  for (const surface of ["environ", "fd", "mem"]) {
    assert.match(probeOutput, new RegExp(`^${target}-${surface}=denied$`, "mu"));
  }
}
for (const capability of ["CapInh", "CapPrm", "CapEff", "CapAmb"]) {
  assert.match(probeOutput, new RegExp(`^${capability}:\\s+0+$`, "mu"));
}
assert.match(probeOutput, /^NoNewPrivs:\s+1$/mu);
assert.match(probeOutput, /^shared-ipc=absent$/mu);
await call("cancel", {
  operation: managedReceipt.operation,
  reason: "procfs isolation fixture cleanup",
});
const managedObservation = await call("observe", {
  cursor: "0",
  operation: managedReceipt.operation,
  wait_ms: 10_000,
});
assert.equal(managedObservation.state, "terminal");
assert.equal(managedObservation.terminal.outcome, "cancelled");
assert.equal(JSON.stringify(managedObservation).includes(secret), false);
await call("cancel", {
  operation: sameBindingReceipt.operation,
  reason: "same-binding isolation fixture cleanup",
});
const sameBindingObservation = await call("observe", {
  cursor: "0",
  operation: sameBindingReceipt.operation,
  wait_ms: 10_000,
});
assert.equal(sameBindingObservation.state, "terminal");
assert.equal(sameBindingObservation.terminal.outcome, "cancelled");

// A shell that never reads stdin must not hold the global idempotency book. Fill its pipe with
// separately identified PIPE_BUF writes until the bounded write reports an honest no-effect
// result, then prove an unrelated execution still accepts input immediately.
const blockedExecution = execution("blocked-stdin-execution-ci", "blocked-stdin", 60_000);
blockedExecution.input.command = "sleep 60";
blockedExecution.input.interactive = true;
sealed(blockedExecution);
const blockedReceipt = await call("execute_sandbox", blockedExecution);
let refusedWrite;
for (let index = 0; index < 64; index += 1) {
  const request = stdinRequest(
    blockedExecution.execution_id,
    `blocked-stdin-write-${index}`,
    "x".repeat(4096),
  );
  const receipt = await call("write_stdin", request);
  if (!receipt.accepted) {
    refusedWrite = { request, receipt };
    break;
  }
}
assert.ok(refusedWrite, "a non-reading shell pipe never reached its bounded full state");
const refusedReplay = await call("write_stdin", refusedWrite.request);
assert.equal(refusedReplay.accepted, false);
assert.equal(refusedReplay.replayed, true);

const readerExecution = execution("reader-stdin-execution-ci", "reader-stdin", 10_000);
readerExecution.input.command = "IFS= read -r value; printf '%s' \"$value\"";
readerExecution.input.interactive = true;
sealed(readerExecution);
const readerReceipt = await call("execute_sandbox", readerExecution);
const readerWrite = stdinRequest(
  readerExecution.execution_id,
  "reader-stdin-write",
  "hello\n",
);
const acceptedWrite = await call("write_stdin", readerWrite);
assert.equal(acceptedWrite.accepted, true);
assert.equal(acceptedWrite.replayed, false);
const acceptedReplay = await call("write_stdin", readerWrite);
assert.equal(acceptedReplay.accepted, true);
assert.equal(acceptedReplay.replayed, true);
const readerPoll = stdinRequest(
  readerExecution.execution_id,
  "reader-stdin-poll",
  "",
);
const polled = await call("write_stdin", readerPoll);
assert.equal(polled.accepted, false);
assert.equal(polled.replayed, false);
assert.equal(polled.observation.operation.operation_id, readerExecution.execution_id);
const readerObservation = await call("observe", {
  cursor: "0",
  operation: readerReceipt.operation,
  wait_ms: 10_000,
});
assert.equal(readerObservation.state, "terminal");
assert.equal(readerObservation.terminal.inline.stdout, "hello");

const eofExecution = sandboxExecution(
  "eof-stdin-execution-ci",
  "cat",
  10_000,
  true,
);
const eofReceipt = await call("execute_sandbox", eofExecution);
const eofData = stdinRequest(
  eofExecution.execution_id,
  "eof-stdin-data",
  "without-newline",
);
assert.equal((await call("write_stdin", eofData)).accepted, true);
const eofClose = stdinRequest(
  eofExecution.execution_id,
  "eof-stdin-close",
  "",
  true,
);
const closed = await call("write_stdin", eofClose);
assert.equal(closed.accepted, true);
assert.equal(closed.replayed, false);
const closedReplay = await call("write_stdin", eofClose);
assert.equal(closedReplay.accepted, true);
assert.equal(closedReplay.replayed, true);
const eofObservation = await call("observe", {
  cursor: "0",
  operation: eofReceipt.operation,
  wait_ms: 10_000,
});
assert.equal(eofObservation.state, "terminal");
assert.equal(eofObservation.terminal.inline.stdout, "without-newline");
await call("cancel", {
  operation: blockedReceipt.operation,
  reason: "full-pipe conformance cleanup",
});
socket.close();

async function waitForFile(path) {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      await readFile(path);
      return;
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  }
  throw new Error(`${path} was not created`);
}

async function postInstall(path, body, contentType = "application/json") {
  const response = await fetch(`${authority}${path}`, {
    method: "POST",
    headers: {
      "content-type": contentType,
      "x-aex-environment-control": controlToken,
    },
    body: contentType === "application/json" ? JSON.stringify(body) : body,
  });
  const result = await response.text();
  assert.equal(response.status, 200, `${path}: ${result}`);
  assert.equal(result.includes(secret), false);
  return JSON.parse(result);
}

function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "number"
      || typeof value === "string") {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  return `{${Object.keys(value).sort().map((key) => (
    `${JSON.stringify(key)}:${canonicalJson(value[key])}`
  )).join(",")}}`;
}
