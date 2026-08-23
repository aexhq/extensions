import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const environmentRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const runner = join(environmentRoot, "image", "tool-runner.mjs");
const contractDigest = "0123456789abcdef".repeat(4);
const secretValue = "runner-secret-value-that-must-not-leak";

async function fixture(directory) {
  const bytes = Buffer.from(`
import { writeFile } from "node:fs/promises";
export default {
  kind: "tool-runtime/v1",
  name: "runner_fixture",
  description: "Environments runner fixture.",
  contractDigest: ${JSON.stringify(contractDigest)},
  requiredEnv: ["RUNNER_SECRET"],
  execute: async (input, context) => {
    await writeFile(process.env.INVOCATION_MARKER, "invoked");
    return {
      input,
      secretLength: process.env.RUNNER_SECRET.length,
      operationId: context.operationId,
      sessionId: context.sessionId,
      workspace: context.workspace,
      hasSignal: context.signal instanceof AbortSignal,
    };
  },
};
`);
  return { bytes, checksum: createHash("sha256").update(bytes).digest("hex") };
}

function request(bundleDigest, digest = contractDigest, operationId = "operation-1", input = { value: "ok" }) {
  return {
    operation_id: operationId,
    session_id: "session-1",
    phase: "execute",
    seal: {
      name: "runner_fixture",
      description: "Environments runner fixture.",
      contract_digest: digest,
      bundle_digest: bundleDigest,
      required_env: ["RUNNER_SECRET"],
    },
    input,
    workspace: "/workspace",
    deadline_ms: Date.now() + 60_000,
    max_output_bytes: 65_536,
  };
}

async function execute(directory, prepared, body) {
  const bundle = join(directory, `${body.operation_id}.bundle.mjs`);
  const marker = join(directory, `${body.operation_id}.invoked.txt`);
  await writeFile(bundle, prepared.bytes);
  const encodedRequest = JSON.stringify(body);
  assert.equal(encodedRequest.includes(secretValue), false);
  const child = spawn(process.execPath, [runner, bundle], {
    env: {
      PATH: process.env.PATH,
      RUNNER_SECRET: secretValue,
      INVOCATION_MARKER: marker,
    },
    stdio: ["pipe", "pipe", "pipe", "pipe"],
  });
  child.stdin.end(encodedRequest);
  const stdout = [];
  const stderr = [];
  const resultFrame = [];
  child.stdout.on("data", (chunk) => stdout.push(chunk));
  child.stderr.on("data", (chunk) => stderr.push(chunk));
  child.stdio[3].on("data", (chunk) => resultFrame.push(chunk));
  const status = await new Promise((resolveStatus, reject) => {
    child.once("error", reject);
    child.once("exit", (code, signal) => resolveStatus({ code, signal }));
  });
  const diagnostics = Buffer.concat([...stdout, ...stderr]).toString("utf8");
  const frame = Buffer.concat(resultFrame);
  assert.ok(frame.length >= 4, `runner must emit a framed result: ${diagnostics}`);
  const resultLength = frame.readUInt32BE(0);
  assert.equal(frame.length, resultLength + 4, "runner must emit exactly one complete result frame");
  const result = JSON.parse(frame.subarray(4).toString("utf8"));
  assert.equal(diagnostics.includes(secretValue), false);
  assert.equal(JSON.stringify(result).includes(secretValue), false);
  return { status, result, marker };
}

test("a prepared Node 22 Tool executes without secret leakage", async () => {
  const directory = await mkdtemp(join(tmpdir(), "environments-runner-"));
  try {
    const prepared = await fixture(directory);
    assert.match(prepared.checksum, /^[0-9a-f]{64}$/u);
    const ran = await execute(directory, prepared, request(prepared.checksum));
    assert.deepEqual(ran.status, { code: 0, signal: null });
    assert.equal(ran.result.ok, true);
    assert.deepEqual(ran.result.output, {
      input: { value: "ok" },
      secretLength: secretValue.length,
      operationId: "operation-1",
      sessionId: "session-1",
      workspace: "/workspace",
      hasSignal: true,
    });
    assert.equal((await readFile(ran.marker, "utf8")), "invoked");
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("bundle and sealed-contract mismatches fail before handler invocation", async () => {
  const directory = await mkdtemp(join(tmpdir(), "environments-runner-mismatch-"));
  try {
    const prepared = await fixture(directory);
    const wrongContract = await execute(
      directory,
      prepared,
      request(prepared.checksum, "f".repeat(64), "wrong-contract-operation"),
    );
    assert.equal(wrongContract.status.code, 1);
    assert.equal(wrongContract.result.ok, false);
    await assert.rejects(stat(wrongContract.marker), { code: "ENOENT" });

    const tampered = { ...prepared, bytes: Buffer.concat([prepared.bytes, Buffer.from("\n//tampered")]) };
    const wrongBundle = await execute(
      directory,
      tampered,
      request(prepared.checksum, contractDigest, "wrong-bundle-operation"),
    );
    assert.equal(wrongBundle.status.code, 1);
    assert.equal(wrongBundle.result.ok, false);
    await assert.rejects(stat(wrongBundle.marker), { code: "ENOENT" });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("concurrent bindings cannot precreate, read, replace, or cross-wire anonymous IPC", async () => {
  const directory = await mkdtemp(join(tmpdir(), "environments-runner-ipc-"));
  try {
    const prepared = await fixture(directory);
    const legacyIpc = join(directory, "tool-ipc");
    await mkdir(legacyIpc);
    const operationIds = ["binding-left-operation", "binding-right-operation"];
    const sentinels = ["left-private-input", "right-private-input"];
    const observedLegacyBytes = [];
    const attacker = (async () => {
      for (let iteration = 0; iteration < 64; iteration += 1) {
        for (const operationId of operationIds) {
          const requestPath = join(legacyIpc, `${operationId}.request.json`);
          const resultPath = join(legacyIpc, `${operationId}.result.json`);
          const replacement = join(legacyIpc, `${operationId}.${iteration}.replacement`);
          await writeFile(requestPath, `attacker-request-${iteration}`);
          observedLegacyBytes.push(await readFile(requestPath, "utf8"));
          await writeFile(replacement, `attacker-result-${iteration}`);
          await rename(replacement, resultPath);
        }
        await new Promise((resolveTurn) => setImmediate(resolveTurn));
      }
    })();
    const [left, right] = await Promise.all([
      execute(
        directory,
        prepared,
        request(prepared.checksum, contractDigest, operationIds[0], { value: sentinels[0] }),
      ),
      execute(
        directory,
        prepared,
        request(prepared.checksum, contractDigest, operationIds[1], { value: sentinels[1] }),
      ),
      attacker,
    ]);
    assert.equal(left.result.output.input.value, sentinels[0]);
    assert.equal(right.result.output.input.value, sentinels[1]);
    assert.equal(JSON.stringify(left.result).includes(sentinels[1]), false);
    assert.equal(JSON.stringify(right.result).includes(sentinels[0]), false);
    assert.equal(observedLegacyBytes.some((value) => sentinels.some((sentinel) => value.includes(sentinel))), false);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
