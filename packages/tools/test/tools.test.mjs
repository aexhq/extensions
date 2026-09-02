import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { createEnvironmentHandler, environment, installExtensionIdentity } from "@aexhq/brain";
import { read } from "../dist/index.mjs";

/** Each official tool's honest declaration: what it operates on, and which kind
 * of program it is. */
const DECLARATIONS = {
  bash: { needs: ["process"], program: "shell" },
  edit: { needs: ["fs"], program: "esm" },
  glob: { needs: ["fs"], program: "esm" },
  grep: { needs: ["process"], program: "esm" },
  ls: { needs: ["fs"], program: "esm" },
  read: { needs: ["fs"], program: "esm" },
  todo: { needs: [], program: "esm" },
  write: { needs: ["fs"], program: "esm" },
};

const artifacts = Object.fromEntries(await Promise.all(Object.keys(DECLARATIONS).map(async (name) => [
  name,
  JSON.parse(await readFile(join(import.meta.dirname, "../dist", `${name}.tool.json`), "utf8")),
])));

let sequence = 0;
const command = (request, attachmentId) => {
  sequence += 1;
  const id = `op_${sequence}`;
  return {
    contract: "environment/v1",
    binding: {},
    operation: { operation_id: id, request_identity: id.padEnd(64, "a"), environment_id: "env_1", session_id: "ses_test", ...(attachmentId === undefined ? {} : { attachment_id: attachmentId }), request },
  };
};

const originalCwd = process.cwd();

/** A hosting Environment rooted in a directory: esm programs run in this
 * process with the root as the working directory, shell programs go to a
 * scripted executor, and every official artifact is attached. */
async function host({ root, shell }) {
  const definition = environment({ resources: { fs: { root }, process: {} } }, (author) => {
    const box = author.open(async () => {
      process.chdir(root);
      return {};
    });
    box.execute.esm({ artifacts: Object.values(artifacts) });
    box.execute.shell(shell ?? (async () => { throw new Error("no shell program expected in this test"); }));
    box.close(async () => undefined);
    return {};
  });
  const handle = createEnvironmentHandler(definition);
  const setup = await handle(command({ type: "setup", configuration: {} }));
  assert.equal(setup.receipt.type, "accepted");
  assert.deepEqual(setup.receipt.runtimes, ["esm", "shell"]);
  const attached = await handle(command({
    type: "attach",
    provisions: Object.values(artifacts).map((artifact) => ({ manifest: artifact.manifest, payload_identity: artifact.manifest.program.identity })),
    bindings: {},
  }, "att_1"));
  assert.equal(attached.receipt.type, "accepted", JSON.stringify(attached.receipt));
  assert.deepEqual(attached.receipt.resources, { fs: { root }, process: {} });
  return async (tool, input) => {
    sequence += 1;
    const invoked = await handle(command({ type: "invoke", call_id: `call_${sequence}`, tool, input, deadline_ms: 30_000 }, "att_1"));
    assert.equal(invoked.receipt.type, "outcome", JSON.stringify(invoked.receipt));
    return invoked.receipt.outcome;
  };
}

const workspace = () => mkdtemp(join(tmpdir(), "aex-tools-"));
const release = async (root) => {
  process.chdir(originalCwd);
  await rm(root, { recursive: true, force: true });
};

test("every manifest declares its honest needs and program kind", () => {
  for (const [name, { needs, program }] of Object.entries(DECLARATIONS)) {
    assert.deepEqual(artifacts[name].manifest.needs, needs, name);
    assert.equal(artifacts[name].manifest.program.kind, program, name);
    assert.equal(artifacts[name].manifest.hosting, undefined, "hosting is not a manifest field");
  }
  assert.equal(artifacts.bash.manifest.program.script, "$command");
  assert.equal(artifacts.bash.payload, "$command", "a shell tool's payload is its script");
});

test("factories demand a placement and mint opaque bound instances", () => {
  assert.throws(() => read(), /placed with \{ env \}/u);
  const boxFactory = environment((author) => {
    const instance = author.open(async () => ({}));
    instance.close(async () => undefined);
    return {};
  });
  installExtensionIdentity(boxFactory, "box");
  const box = boxFactory();
  assert.deepEqual(Object.keys(read({ env: box })), [], "a bound tool exposes no surface");
});

test("file tools work on the workspace through node's own fs", async () => {
  const root = await workspace();
  try {
    const invoke = await host({ root });
    assert.deepEqual(await invoke("write", { path: "notes/hello.txt", content: "hello world" }), {
      status: "ok",
      value: { path: "notes/hello.txt", bytes: 11 },
    });
    assert.equal(await readFile(join(root, "notes/hello.txt"), "utf8"), "hello world", "relative paths land in the workspace");
    const first = await invoke("read", { path: "notes/hello.txt" });
    assert.equal(first.value.content, "hello world");
    assert.equal(first.value.truncated, false);
    assert.equal((await invoke("edit", { path: "notes/hello.txt", old_text: "world", new_text: "aex" })).status, "ok");
    assert.equal((await invoke("read", { path: "notes/hello.txt" })).value.content, "hello aex");
    const listed = await invoke("ls", { path: "." });
    assert.deepEqual(listed.value.entries, [{ name: "notes", kind: "dir" }]);
    assert.deepEqual((await invoke("glob", { pattern: "**/*.txt" })).value, { paths: ["notes/hello.txt"], truncated: false });
    assert.deepEqual((await invoke("glob", { pattern: "*.txt" })).value.paths, [], "a single-segment glob stays at the top level");
    assert.deepEqual((await invoke("glob", { pattern: "notes/h?llo.txt" })).value.paths, ["notes/hello.txt"]);
    const missing = await invoke("read", { path: "nope.txt" });
    assert.equal(missing.status, "error", "a platform error is an ordinary tool error");
  } finally {
    await release(root);
  }
});

test("read applies offset and limit after node returns the file", async () => {
  const root = await workspace();
  try {
    await writeFile(join(root, "data.txt"), "0123456789");
    const invoke = await host({ root });
    const windowed = await invoke("read", { path: "data.txt", offset: 2, limit: 3 });
    assert.deepEqual(windowed.value, { content: "234", bytes: 3, truncated: true });
    await writeFile(join(root, "binary.bin"), Buffer.from([104, 105, 0, 106]));
    const binary = await invoke("read", { path: "binary.bin" });
    assert.equal(binary.status, "error");
    assert.match(binary.error.message, /is binary/u);
  } finally {
    await release(root);
  }
});

test("bash is a shell program: the command reaches the executor as the script", async () => {
  const root = await workspace();
  const scripts = [];
  try {
    const invoke = await host({
      root,
      shell: async (context, script) => {
        scripts.push({ script, callId: context.callId });
        return { exit_code: 0, stdout: `ran: ${script}`, stderr: "" };
      },
    });
    const ran = await invoke("bash", { command: "echo hi" });
    assert.deepEqual(ran.value, { exit_code: 0, stdout: "ran: echo hi", stderr: "" });
    assert.equal(scripts[0].script, "echo hi");
    const rejected = await invoke("bash", { command: "" });
    assert.equal(rejected.status, "ok", "shell input is substituted, not schema-validated in the host");
  } finally {
    await release(root);
  }
});

const ripgrep = (() => { try { execFileSync("rg", ["--version"], { stdio: "ignore" }); return true; } catch { return false; } })();

test("grep drives ripgrep through a process and keeps no-match and failure semantics", { skip: ripgrep ? false : "ripgrep is not installed here" }, async () => {
  const root = await workspace();
  try {
    await writeFile(join(root, "a.txt"), "needle's here\nnothing\n");
    await writeFile(join(root, "b.txt"), "another needle's\n");
    const invoke = await host({ root });
    assert.deepEqual((await invoke("grep", { pattern: "needle's" })).value, { matches: ["a.txt:1:needle's here", "b.txt:1:another needle's"], truncated: false });
    assert.deepEqual((await invoke("grep", { pattern: "absent" })).value, { matches: [], truncated: false });
    const failed = await invoke("grep", { pattern: "(" });
    assert.equal(failed.status, "error");
    assert.match(failed.error.message, /regex|parse|unclosed/iu);
  } finally {
    await release(root);
  }
});

test("todo is pure: no resource, list kept in the hosted module", async () => {
  const root = await workspace();
  try {
    const invoke = await host({ root });
    await invoke("todo", { action: "set", items: [{ text: "ship it", done: false }] });
    assert.deepEqual((await invoke("todo", { action: "get" })).value, { items: [{ text: "ship it", done: false }] });
  } finally {
    await release(root);
  }
});
