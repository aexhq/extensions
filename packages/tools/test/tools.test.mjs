import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { definitions, handlers, read } from "../index.mjs";

test("publishes model definitions separately from Environment-side handlers", () => {
  assert.deepEqual(Object.keys(definitions), ["bash", "edit", "glob", "grep", "ls", "read", "todo", "write"]);
  assert.equal(read().definition.name, "read");
  assert.equal(read().remoteToolId, "read");
  assert.equal(typeof handlers.read, "function");
  assert.equal("execute" in read(), false);
});

test("executes a Tool only when an Environment invokes its handler", async () => {
  const workspace = await mkdtemp(join(tmpdir(), "aex-tools-"));
  try {
    await writeFile(join(workspace, "hello.txt"), "hello");
    const output = await handlers.read(
      { path: "hello.txt" },
      { workspace, deadlineMs: Date.now() + 1_000, signal: new AbortController().signal, grant: {} },
    );
    assert.equal(output.content, "hello");
  } finally {
    await rm(workspace, { recursive: true, force: true });
  }
});

test("rejects a path outside the Environment workspace", async () => {
  await assert.rejects(
    handlers.read(
      { path: "../outside" },
      { workspace: tmpdir(), deadlineMs: Date.now() + 1_000, signal: new AbortController().signal, grant: {} },
    ),
    /path escapes/u,
  );
});
