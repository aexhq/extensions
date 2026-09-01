import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";

import { clamp, createEnvironmentHandler, environment, installExtensionIdentity } from "@aexhq/brain";
import { read } from "../dist/index.mjs";

const REQUIRES = {
  bash: ["exec"],
  edit: ["fs"],
  glob: ["fs"],
  grep: ["exec"],
  ls: ["fs"],
  read: ["fs"],
  todo: [],
  write: ["fs"],
};

const artifacts = Object.fromEntries(await Promise.all(Object.keys(REQUIRES).map(async (name) => [
  name,
  JSON.parse(await readFile(join(import.meta.dirname, "../dist", `${name}.tool.json`), "utf8")),
])));

let sequence = 0;
const command = (request, attachmentId) => {
  sequence += 1;
  const id = `op_${sequence}`;
  return {
    contract: "environment/v2",
    binding: {},
    operation: { operation_id: id, request_identity: id.padEnd(64, "a"), environment_id: "env_1", session_id: "ses_test", ...(attachmentId === undefined ? {} : { attachment_id: attachmentId }), request },
  };
};

/** A hosting Environment with a real node-fs provider rooted in a directory and
 * a scripted exec provider, attaching every official tool artifact. */
async function host({ root, exec }) {
  const definition = environment((author) => {
    const box = author.open(async () => ({}));
    box.run(async () => { throw new Error("every official tool is hosted; nothing reaches the legacy run handler"); });
    box.close(async () => undefined);
    box.provide.exec(({ grants }) => ({ run: (cmd, opts) => exec(cmd, clamp(opts, grants.exec)) }));
    box.provide.fs(({ grants }) => ({
      read: (path) => readFile(clamp.path(grants.fs.root, path)),
      write: async (path, data) => {
        const target = clamp.path(grants.fs.root, path);
        await mkdir(dirname(target), { recursive: true });
        await writeFile(target, data);
      },
      list: async (path) => (await readdir(clamp.path(grants.fs.root, path), { withFileTypes: true }))
        .map((entry) => ({ name: entry.name, kind: entry.isDirectory() ? "dir" : "file" })),
    }));
    box.host.esm({ artifacts: Object.values(artifacts) });
    return {};
  });
  const handle = createEnvironmentHandler(definition);
  assert.equal((await handle(command({ type: "setup", configuration: {} }))).receipt.type, "accepted");
  const attached = await handle(command({
    type: "attach",
    grants: { exec: { timeout_ms_max: 60_000 }, fs: { root } },
    provisions: Object.values(artifacts).map((artifact) => ({ manifest: artifact.manifest, payload_identity: artifact.manifest.payload.identity })),
    bindings: {},
  }, "att_1"));
  assert.equal(attached.receipt.type, "accepted", JSON.stringify(attached.receipt));
  assert.deepEqual(attached.receipt.provides, ["exec", "fs"]);
  return async (tool, input) => {
    sequence += 1;
    const invoked = await handle(command({ type: "invoke", call_id: `call_${sequence}`, tool, input, deadline_ms: 30_000 }, "att_1"));
    assert.equal(invoked.receipt.type, "outcome", JSON.stringify(invoked.receipt));
    return invoked.receipt.outcome;
  };
}

const workspace = () => mkdtemp(join(tmpdir(), "aex-tools-"));
const noExec = () => { throw new Error("no exec expected in this test"); };

test("every manifest declares its honest capability requirements", () => {
  for (const [name, requires] of Object.entries(REQUIRES)) {
    assert.deepEqual(artifacts[name].manifest.requires, requires, name);
    assert.equal(artifacts[name].manifest.hosting, "provisioned");
    assert.equal(artifacts[name].manifest.payload.kind, "esm");
  }
});

test("factories demand a placement and mint opaque bound instances", () => {
  assert.throws(() => read(), /placed with \{ env \}/u);
  const boxFactory = environment((author) => {
    const instance = author.open(async () => ({}));
    instance.run(async () => undefined);
    instance.close(async () => undefined);
    return {};
  });
  installExtensionIdentity(boxFactory, "box");
  const box = boxFactory();
  assert.deepEqual(Object.keys(read({ env: box })), [], "a bound tool exposes no surface");
});

test("file tools round-trip through the environment's fs provider", async () => {
  const root = await workspace();
  try {
    const invoke = await host({ root, exec: noExec });
    assert.deepEqual(await invoke("write", { path: "notes/hello.txt", content: "hello world" }), {
      status: "ok",
      value: { path: "notes/hello.txt", bytes: 11 },
    });
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
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("path escapes stop at the provider, not in the tool", async () => {
  const root = await workspace();
  try {
    const invoke = await host({ root, exec: noExec });
    const escaped = await invoke("read", { path: "../outside" });
    assert.equal(escaped.status, "error");
    assert.equal(escaped.error.code, "path_escape");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("read applies offset and limit after the fs handle returns the file", async () => {
  const root = await workspace();
  try {
    await writeFile(join(root, "data.txt"), "0123456789");
    const invoke = await host({ root, exec: noExec });
    const windowed = await invoke("read", { path: "data.txt", offset: 2, limit: 3 });
    assert.deepEqual(windowed.value, { content: "234", bytes: 3, truncated: true });
    await writeFile(join(root, "binary.bin"), Buffer.from([104, 105, 0, 106]));
    const binary = await invoke("read", { path: "binary.bin" });
    assert.equal(binary.status, "error");
    assert.match(binary.error.message, /is binary/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("bash runs through the exec handle with the granted timeout clamp", async () => {
  const root = await workspace();
  const calls = [];
  try {
    const invoke = await host({
      root,
      exec: async (cmd, opts) => {
        calls.push({ cmd, opts });
        return { exitCode: 0, stdout: `ran: ${cmd}`, stderr: "" };
      },
    });
    const ran = await invoke("bash", { command: "echo hi", timeout_ms: 999_999_999 });
    assert.deepEqual(ran.value, { exit_code: 0, stdout: "ran: echo hi", stderr: "" });
    assert.equal(calls[0].opts.timeoutMs, 60_000, "the grant clamps the requested timeout");
    await invoke("bash", { command: "pwd", cwd: "/elsewhere" });
    assert.equal(calls[1].opts.cwd, "/elsewhere");
    assert.equal(calls[1].opts.timeoutMs, 60_000, "an absent request still gets the granted maximum");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("grep drives ripgrep over exec and keeps no-match and failure semantics", async () => {
  const root = await workspace();
  try {
    let scripted;
    const invoke = await host({ root, exec: async (cmd) => scripted(cmd) });
    scripted = (cmd) => {
      assert.match(cmd, /^rg --line-number --no-heading --color never --regexp 'needle'\\''s' -- '\.'$/u);
      return { exitCode: 0, stdout: "a.txt:1:x\nb.txt:2:y\n", stderr: "" };
    };
    assert.deepEqual((await invoke("grep", { pattern: "needle's" })).value, { matches: ["a.txt:1:x", "b.txt:2:y"], truncated: false });
    scripted = () => ({ exitCode: 1, stdout: "", stderr: "" });
    assert.deepEqual((await invoke("grep", { pattern: "nothing" })).value, { matches: [], truncated: false });
    scripted = () => ({ exitCode: 2, stdout: "", stderr: "rg: bad pattern" });
    const failed = await invoke("grep", { pattern: "(" });
    assert.equal(failed.status, "error");
    assert.match(failed.error.message, /bad pattern/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("todo is pure: no capability, list kept in the hosted module", async () => {
  const root = await workspace();
  try {
    const invoke = await host({ root, exec: noExec });
    await invoke("todo", { action: "set", items: [{ text: "ship it", done: false }] });
    assert.deepEqual((await invoke("todo", { action: "get" })).value, { items: [{ text: "ship it", done: false }] });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
