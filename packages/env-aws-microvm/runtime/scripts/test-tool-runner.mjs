import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync, spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const environmentRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const runner = join(environmentRoot, "image", "tool-runner.mjs");
const runtime = (name) => resolve(environmentRoot, `../../tools/dist/runtime/${name}.mjs`);
const registry = JSON.parse(await readFile(resolve(environmentRoot, "../../tools/dist/runtime/registry.json"), "utf8"));

async function fixture(name, path = runtime(name)) {
  const bytes = await readFile(path);
  return {
    name,
    path,
    bytes,
    executeDigest: createHash("sha256").update(bytes).digest("hex"),
    contractDigest: registry[name].contract_digest,
  };
}

function request(prepared, directory, operationId, input) {
  return {
    operation_id: operationId,
    session_id: "session-1",
    phase: "execute",
    seal: {
      name: prepared.name,
      contract_digest: prepared.contractDigest,
      execute_digest: prepared.executeDigest,
      required_env: [],
    },
    input: { input, options: {} },
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
  return { status, result: JSON.parse(frame.subarray(4).toString("utf8")), diagnostics: Buffer.concat(diagnostics).toString("utf8") };
}

test("the published Tool runtime executes through the Environment runner", async () => {
  const directory = await mkdtemp(join(tmpdir(), "environment-runner-"));
  try {
    const prepared = await fixture("write");
    const ran = await execute(prepared.path, request(prepared, directory, "operation-1", { path: "operation-1.txt", content: "AEX_RUNNER_OK" }));
    assert.deepEqual(ran.status, { code: 0, signal: null }, `${ran.diagnostics}${JSON.stringify(ran.result)}`);
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
    const path = join(directory, "write.mjs");
    await writeFile(path, await readFile(runtime("write")));
    const prepared = await fixture("write", path);
    const input = { path: "mismatch.txt", content: "MUST_NOT_RUN" };
    const body = request(prepared, directory, "wrong-contract", input);
    body.seal.contract_digest = "f".repeat(64);
    const wrongContract = await execute(prepared.path, body);
    assert.equal(wrongContract.status.code, 1);
    assert.equal(wrongContract.result.ok, false);
    await writeFile(prepared.path, Buffer.concat([prepared.bytes, Buffer.from("\n// tampered")]));
    const tampered = await execute(prepared.path, request(prepared, directory, "wrong-bundle", input));
    assert.equal(tampered.status.code, 1);
    assert.equal(tampered.result.ok, false);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("concurrent operations remain isolated on anonymous result channels", async () => {
  const directory = await mkdtemp(join(tmpdir(), "environment-runner-concurrent-"));
  try {
    const prepared = await fixture("write");
    const [left, right] = await Promise.all([
      execute(prepared.path, request(prepared, directory, "left", { path: "left.txt", content: "LEFT_ONLY" })),
      execute(prepared.path, request(prepared, directory, "right", { path: "right.txt", content: "RIGHT_ONLY" })),
    ]);
    assert.equal(left.result.output.path, "left.txt");
    assert.equal(right.result.output.path, "right.txt");
    assert.equal(await readFile(join(directory, "left.txt"), "utf8"), "LEFT_ONLY");
    assert.equal(await readFile(join(directory, "right.txt"), "utf8"), "RIGHT_ONLY");
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

async function invoke(name, directory, operationId, input) {
  const prepared = await fixture(name);
  const ran = await execute(prepared.path, request(prepared, directory, operationId, input));
  assert.deepEqual(ran.status, { code: 0, signal: null }, `${ran.diagnostics}${JSON.stringify(ran.result)}`);
  assert.equal(ran.result.ok, true, `${ran.diagnostics}${JSON.stringify(ran.result)}`);
  return ran.result.output;
}

async function invokeFailure(name, directory, operationId, input) {
  const prepared = await fixture(name);
  const ran = await execute(prepared.path, request(prepared, directory, operationId, input));
  assert.equal(ran.status.code, 1);
  assert.equal(ran.result.ok, false);
  return ran.result.error;
}

test("all filesystem Tool bundles preserve their behavior across fresh runner processes", async () => {
  const directory = await mkdtemp(join(tmpdir(), "environment-runner-tools-"));
  try {
    assert.deepEqual(await invoke("write", directory, "write", { path: "notes/hello.txt", content: "hello world" }), {
      path: "notes/hello.txt",
      bytes: 11,
    });
    assert.deepEqual(await invoke("read", directory, "read", { path: "notes/hello.txt" }), {
      content: "hello world",
      bytes: 11,
      truncated: false,
    });
    assert.deepEqual(await invoke("read", directory, "read-window", { path: "notes/hello.txt", offset: 6, limit: 3 }), {
      content: "wor",
      bytes: 3,
      truncated: true,
    });
    assert.deepEqual(await invoke("edit", directory, "edit", { path: "notes/hello.txt", old_text: "world", new_text: "aex" }), {
      path: "notes/hello.txt",
      replacements: 1,
    });
    assert.deepEqual(await invoke("ls", directory, "ls", { path: "notes" }), {
      entries: [{ name: "hello.txt", kind: "file" }],
      truncated: false,
    });
    assert.deepEqual(await invoke("glob", directory, "glob", { pattern: "**/*.txt" }), {
      paths: ["notes/hello.txt"],
      truncated: false,
    });
    assert.deepEqual(await invoke("glob", directory, "glob-top", { pattern: "*.txt" }), {
      paths: [],
      truncated: false,
    });
    assert.deepEqual(await invoke("glob", directory, "glob-question", { pattern: "notes/h?llo.txt" }), {
      paths: ["notes/hello.txt"],
      truncated: false,
    });
    assert.match(await invokeFailure("read", directory, "read-missing", { path: "missing.txt" }), /ENOENT/u);
    assert.match(await invokeFailure("read", directory, "read-outside", { path: "../outside.txt" }), /outside the Environment workspace/u);
    await writeFile(join(directory, "binary.bin"), Buffer.from([104, 105, 0, 106]));
    assert.match(await invokeFailure("read", directory, "read-binary", { path: "binary.bin" }), /is binary/u);
    await writeFile(join(directory, "duplicate.txt"), "same same");
    assert.match(
      await invokeFailure("edit", directory, "edit-duplicate", { path: "duplicate.txt", old_text: "same", new_text: "new" }),
      /more than once/u,
    );
    assert.deepEqual(await invoke("todo", directory, "todo-set", { action: "set", items: [{ text: "ship", done: false }] }), {
      items: [{ text: "ship", done: false }],
    });
    assert.deepEqual(await invoke("todo", directory, "todo-get", { action: "get" }), {
      items: [{ text: "ship", done: false }],
    });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

const has = (command, args) => {
  try {
    execFileSync(command, args, { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
};

test("the grep bundle drives ripgrep", async () => {
  const directory = await mkdtemp(join(tmpdir(), "environment-runner-grep-"));
  try {
    if (!has("rg", ["--version"])) {
      const prepared = await fixture("grep");
      const ran = await execute(prepared.path, request(prepared, directory, "grep-invalid", { pattern: "" }));
      assert.equal(ran.status.code, 1);
      assert.equal(ran.result.ok, false);
      return;
    }
    await writeFile(join(directory, "a.txt"), "needle\n");
    assert.deepEqual(await invoke("grep", directory, "grep", { pattern: "needle" }), {
      matches: [`${process.platform === "win32" ? ".\\" : ""}a.txt:1:needle`],
      truncated: false,
    });
    assert.deepEqual(await invoke("grep", directory, "grep-empty", { pattern: "absent" }), {
      matches: [],
      truncated: false,
    });
    assert.match(await invokeFailure("grep", directory, "grep-invalid", { pattern: "(" }), /regex|parse|unclosed/iu);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("the bash bundle executes in its workspace", async () => {
  const directory = await mkdtemp(join(tmpdir(), "environment-runner-bash-"));
  try {
    if (!has("bash", ["--version"])) {
      const prepared = await fixture("bash");
      const ran = await execute(prepared.path, request(prepared, directory, "bash-invalid", { command: "" }));
      assert.equal(ran.status.code, 1);
      assert.equal(ran.result.ok, false);
      return;
    }
    assert.deepEqual(await invoke("bash", directory, "bash", { command: "printf AEX_BASH_OK" }), {
      exit_code: 0,
      stdout: "AEX_BASH_OK",
      stderr: "",
    });
    assert.deepEqual(await invoke("bash", directory, "bash-failed", { command: "printf AEX_BASH_ERROR >&2; exit 7" }), {
      exit_code: 7,
      stdout: "",
      stderr: "AEX_BASH_ERROR",
    });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
