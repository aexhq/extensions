import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

import { awsMicroVm } from "./dist/index.mjs";

test("creates an opaque Environment with its own methods", async () => {
  const vm = awsMicroVm({ region: "eu-west-2", idleSeconds: 30, maximumSeconds: 600 });
  assert.deepEqual(Object.keys(vm), ["suspend"]);
  await assert.rejects(vm.suspend(), /only while its session is attached/u);
});

test("validates provider options through the declared schema", () => {
  assert.doesNotThrow(() => awsMicroVm());
  assert.throws(() => awsMicroVm({ region: "" }), /Too small/u);
  assert.throws(() => awsMicroVm({ idleSeconds: 0 }), /Too small/u);
  assert.throws(() => awsMicroVm({ idleSeconds: 61, maximumSeconds: 60 }), /cannot exceed/u);
  assert.throws(() => awsMicroVm({ typo: true }), /Unrecognized key/u);
});

// An esm program on node's own APIs: relative paths land in the workspace
// because the environment moves there.
const probePayload = `import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
export default {
  kind: "brain.provisioned-tool/v1",
  parseInput: (input) => input,
  async run(input, context) {
    await mkdir("out", { recursive: true });
    await writeFile("out/answer.txt", "42");
    const listed = await readdir("out");
    const answer = await readFile("out/answer.txt", "utf8");
    return { listed, answer, cwd: process.cwd(), keys: Object.keys(context).sort() };
  },
};`;

const identityOf = (payload) => createHash("sha256").update(payload).digest("hex");
const probeManifest = {
  name: "probe",
  description: "exercise node's fs from the workspace",
  input_schema: { type: "object" },
  needs: ["fs"],
  binding_names: [],
  program: { kind: "esm", identity: identityOf(probePayload) },
};
const shellManifest = (name, script) => ({ name, description: "one shell program", input_schema: { type: "object" }, needs: ["process"], binding_names: [], program: { kind: "shell", identity: identityOf(script), script } });
const provisionOf = (manifest) => ({ manifest, payload_identity: manifest.program.identity });

let sequence = 0;
const command = (request, attachmentId) => {
  sequence += 1;
  const id = `op_${sequence}`;
  return {
    contract: "environment/v1",
    binding: {},
    operation: { operation_id: id, request_identity: id.padEnd(64, "a"), environment_id: "env_vm", session_id: "ses_test", ...(attachmentId === undefined ? {} : { attachment_id: attachmentId }), request },
  };
};

test("launches esm and shell programs in the workspace and declares what it enforces", async () => {
  const artifactDirectory = await mkdtemp(join(tmpdir(), "aex-microvm-artifacts-"));
  const root = await mkdtemp(join(tmpdir(), "aex-microvm-workspace-"));
  const originalCwd = process.cwd();
  try {
    await writeFile(join(artifactDirectory, "probe.tool.json"), JSON.stringify({ manifest: probeManifest, payload: probePayload }));
    process.env.AEX_TOOL_ARTIFACT_DIR = artifactDirectory;
    process.env.AEX_WORKSPACE_ROOT = root;
    const handle = (await import(pathToFileURL(join(import.meta.dirname, "dist/runtime/awsMicroVm.mjs")).href)).default;

    const setup = await handle(command({ type: "setup", configuration: { driver: "aws-microvm" } }));
    assert.equal(setup.receipt.type, "accepted");
    assert.deepEqual(setup.receipt.runtimes, ["esm", "shell"], "the receipt reports honest runtimes");
    assert.deepEqual(setup.receipt.resources, { fs: { root }, process: { output_bytes_max: 1024 * 1024 } }, "the receipt declares the workspace and the process bounds");

    const attached = await handle(command({
      type: "attach",
      provisions: [provisionOf(probeManifest), provisionOf(shellManifest("echo", "printf hi")), provisionOf(shellManifest("sleeper", "sleep 30")), provisionOf(shellManifest("sub", "printf $text"))],
      bindings: {},
    }, "att_1"));
    assert.equal(attached.receipt.type, "accepted", JSON.stringify(attached.receipt));

    const probed = await handle(command({ type: "invoke", call_id: "call_probe", tool: "probe", input: {}, deadline_ms: 30_000 }, "att_1"));
    assert.equal(probed.receipt.type, "outcome", JSON.stringify(probed.receipt));
    assert.equal(probed.receipt.outcome.status, "ok", JSON.stringify(probed.receipt.outcome));
    const value = probed.receipt.outcome.value;
    assert.deepEqual(value.listed, ["answer.txt"]);
    assert.equal(value.answer, "42");
    assert.equal(await readFile(join(root, "out", "answer.txt"), "utf8"), "42", "the program wrote into the workspace");
    assert.deepEqual(value.keys, ["bindings", "callId", "deadline", "progress", "requestId", "signal"], "no handles: the program used node's fs");

    const echoed = await handle(command({ type: "invoke", call_id: "call_echo", tool: "echo", input: {}, deadline_ms: 30_000 }, "att_1"));
    assert.equal(echoed.receipt.outcome.status, "ok", JSON.stringify(echoed.receipt.outcome));
    assert.equal(echoed.receipt.outcome.value.exit_code, 0);
    assert.equal(echoed.receipt.outcome.value.stdout, "hi");

    const substituted = await handle(command({ type: "invoke", call_id: "call_sub", tool: "sub", input: { text: "there" }, deadline_ms: 30_000 }, "att_1"));
    assert.equal(substituted.receipt.outcome.value.stdout, "there", "input references are substituted before the shell runs");

    const slept = await handle(command({ type: "invoke", call_id: "call_sleep", tool: "sleeper", input: {}, deadline_ms: 500 }, "att_1"));
    assert.equal(slept.receipt.outcome.status, "timeout", "the call's deadline kills a program that outlives it");

    const unknown = await handle(command({ type: "invoke", call_id: "call_unknown", tool: "nothing", input: {}, deadline_ms: 1_000 }, "att_1"));
    assert.equal(unknown.receipt.type, "failure", "a tool that was never provisioned has nothing to run");
  } finally {
    process.chdir(originalCwd);
    delete process.env.AEX_TOOL_ARTIFACT_DIR;
    delete process.env.AEX_WORKSPACE_ROOT;
    await rm(artifactDirectory, { recursive: true, force: true });
    await rm(root, { recursive: true, force: true });
  }
});
