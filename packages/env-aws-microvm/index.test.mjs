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

const probePayload = `export default {
  kind: "brain.provisioned-tool/v1",
  parseInput: (input) => input,
  async run(input, context) {
    await context.fs.write("out/answer.txt", "42");
    const listed = await context.fs.list("out");
    const answer = new TextDecoder().decode(await context.fs.read("out/answer.txt"));
    const echoed = await context.exec.run("printf hi", { timeoutMs: 999_999_999 });
    const slept = await context.exec.run("sleep 30");
    return { listed, answer, echoed, sleptExit: slept.exitCode };
  },
};`;

const probeManifest = {
  name: "probe",
  description: "exercise the exec and fs providers",
  input_schema: { type: "object" },
  requires: ["exec", "fs"],
  binding_names: [],
  hosting: "provisioned",
  payload: { kind: "esm", identity: createHash("sha256").update(probePayload).digest("hex") },
};

let sequence = 0;
const command = (request, attachmentId) => {
  sequence += 1;
  const id = `op_${sequence}`;
  return {
    contract: "environment/v2",
    binding: {},
    operation: { operation_id: id, request_identity: id.padEnd(64, "a"), environment_id: "env_vm", session_id: "ses_test", ...(attachmentId === undefined ? {} : { attachment_id: attachmentId }), request },
  };
};

test("hosts provisioned tools on the VM exec and workspace fs providers", async () => {
  const artifactDirectory = await mkdtemp(join(tmpdir(), "aex-microvm-artifacts-"));
  const root = await mkdtemp(join(tmpdir(), "aex-microvm-workspace-"));
  try {
    await writeFile(join(artifactDirectory, "probe.tool.json"), JSON.stringify({ manifest: probeManifest, payload: probePayload }));
    process.env.AEX_TOOL_ARTIFACT_DIR = artifactDirectory;
    const handle = (await import(pathToFileURL(join(import.meta.dirname, "dist/runtime/awsMicroVm.mjs")).href)).default;

    const setup = await handle(command({ type: "setup", configuration: { driver: "aws-microvm" } }));
    assert.equal(setup.receipt.type, "accepted");
    assert.deepEqual(setup.receipt.provides, ["exec", "fs"], "the receipt reports honest provides");

    const attached = await handle(command({
      type: "attach",
      grants: { exec: { timeout_ms_max: 500 }, fs: { root } },
      provisions: [{ manifest: probeManifest, payload_identity: probeManifest.payload.identity }],
      bindings: {},
    }, "att_1"));
    assert.equal(attached.receipt.type, "accepted", JSON.stringify(attached.receipt));

    const invoked = await handle(command({ type: "invoke", call_id: "call_probe", tool: "probe", input: {}, deadline_ms: 30_000 }, "att_1"));
    assert.equal(invoked.receipt.type, "outcome", JSON.stringify(invoked.receipt));
    assert.equal(invoked.receipt.outcome.status, "ok", JSON.stringify(invoked.receipt.outcome));
    const value = invoked.receipt.outcome.value;
    assert.deepEqual(value.listed, [{ name: "answer.txt", kind: "file" }]);
    assert.equal(value.answer, "42");
    assert.equal(value.echoed.exitCode, 0);
    assert.equal(value.echoed.stdout, "hi");
    assert.notEqual(value.sleptExit, 0, "the granted timeout ceiling kills a command that outlives it");
    assert.equal(await readFile(join(root, "out", "answer.txt"), "utf8"), "42");

    const ungranted = await handle(command({
      type: "attach",
      grants: { exec: { timeout_ms_max: 500 } },
      provisions: [{ manifest: probeManifest, payload_identity: probeManifest.payload.identity }],
      bindings: {},
    }, "att_2"));
    assert.equal(ungranted.receipt.type, "accepted");
    const denied = await handle(command({ type: "invoke", call_id: "call_denied", tool: "probe", input: {}, deadline_ms: 30_000 }, "att_2"));
    assert.equal(denied.receipt.outcome.status, "error");
    assert.equal(denied.receipt.outcome.error.code, "not_granted", "without an fs grant the provider denies by default");
  } finally {
    delete process.env.AEX_TOOL_ARTIFACT_DIR;
    await rm(artifactDirectory, { recursive: true, force: true });
    await rm(root, { recursive: true, force: true });
  }
});
