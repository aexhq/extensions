import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { definitions } from "@aexhq/tools";

const environmentRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const runner = join(environmentRoot, "image", "tool-runner.mjs");
const runtime = resolve(environmentRoot, "../../tools/dist/runtime/write.mjs");

async function fixture(directory) {
  const bytes = await readFile(runtime);
  const path = join(directory, "write.mjs");
  await writeFile(path, bytes);
  return {
    path,
    bytes,
    executeDigest: createHash("sha256").update(bytes).digest("hex"),
    contractDigest: JSON.parse(await readFile(resolve(environmentRoot, "../../tools/dist/runtime/registry.json"), "utf8")).write.contract_digest,
  };
}

function request(prepared, directory, operationId, content = "AEX_RUNNER_OK") {
  return {
    operation_id: operationId,
    session_id: "session-1",
    phase: "execute",
    seal: {
      name: "write",
      description: definitions.write.definition.description,
      contract_digest: prepared.contractDigest,
      execute_digest: prepared.executeDigest,
      required_env: [],
    },
    input: { path: `${operationId}.txt`, content },
    workspace: directory,
    deadline_ms: Date.now() + 60_000,
    max_output_bytes: 65_536,
  };
}

async function execute(bundle, body) {
  const child = spawn(process.execPath, [runner, bundle], {
    env: { PATH: process.env.PATH },
    stdio: ["pipe", "pipe", "pipe", "pipe"],
  });
  child.stdin.end(JSON.stringify(body));
  const diagnostics = [];
  const resultFrame = [];
  child.stdout.on("data", (chunk) => diagnostics.push(chunk));
  child.stderr.on("data", (chunk) => diagnostics.push(chunk));
  child.stdio[3].on("data", (chunk) => resultFrame.push(chunk));
  const status = await new Promise((resolveStatus, reject) => {
    child.once("error", reject);
    child.once("exit", (code, signal) => resolveStatus({ code, signal }));
  });
  const frame = Buffer.concat(resultFrame);
  assert.ok(frame.length >= 4, `runner emitted no result: ${Buffer.concat(diagnostics).toString("utf8")}`);
  assert.equal(frame.length, frame.readUInt32BE(0) + 4);
  return { status, result: JSON.parse(frame.subarray(4).toString("utf8")) };
}

test("the published Tool runtime executes through the Environment runner", async () => {
  const directory = await mkdtemp(join(tmpdir(), "environment-runner-"));
  try {
    const prepared = await fixture(directory);
    const ran = await execute(prepared.path, request(prepared, directory, "operation-1"));
    assert.deepEqual(ran.status, { code: 0, signal: null });
    assert.equal(ran.result.ok, true);
    assert.deepEqual(ran.result.output, { path: "operation-1.txt", bytes: 13 });
    assert.equal(await readFile(join(directory, "operation-1.txt"), "utf8"), "AEX_RUNNER_OK");
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("sealed digest mismatches fail before Tool execution", async () => {
  const directory = await mkdtemp(join(tmpdir(), "environment-runner-mismatch-"));
  try {
    const prepared = await fixture(directory);
    const body = request(prepared, directory, "wrong-contract");
    body.seal.contract_digest = "f".repeat(64);
    const wrongContract = await execute(prepared.path, body);
    assert.equal(wrongContract.status.code, 1);
    assert.equal(wrongContract.result.ok, false);
    await writeFile(prepared.path, Buffer.concat([prepared.bytes, Buffer.from("\n// tampered")]));
    const tampered = await execute(prepared.path, request(prepared, directory, "wrong-bundle"));
    assert.equal(tampered.status.code, 1);
    assert.equal(tampered.result.ok, false);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("concurrent operations remain isolated on anonymous result channels", async () => {
  const directory = await mkdtemp(join(tmpdir(), "environment-runner-concurrent-"));
  try {
    const prepared = await fixture(directory);
    const [left, right] = await Promise.all([
      execute(prepared.path, request(prepared, directory, "left", "LEFT_ONLY")),
      execute(prepared.path, request(prepared, directory, "right", "RIGHT_ONLY")),
    ]);
    assert.equal(left.result.output.path, "left.txt");
    assert.equal(right.result.output.path, "right.txt");
    assert.equal(await readFile(join(directory, "left.txt"), "utf8"), "LEFT_ONLY");
    assert.equal(await readFile(join(directory, "right.txt"), "utf8"), "RIGHT_ONLY");
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
