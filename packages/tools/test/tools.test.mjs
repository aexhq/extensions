import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

import { environment, installExtensionIdentity } from "@aexhq/brain";
import { read } from "../dist/index.mjs";

const workspace = environment((author) => {
  const instance = author.open(async () => ({}));
  instance.run(async () => undefined);
  instance.close(async () => undefined);
  return {};
});
installExtensionIdentity(workspace, "workspace");

test("publishes generated model factories without Environment-side handlers", () => {
  assert.deepEqual(Object.keys(read()), ["useIn"]);
  assert.deepEqual(Object.keys(read().useIn(workspace())), []);
  assert.equal("execute" in read(), false);
});

test("keeps model-visible descriptions out of the Environment runtime", async () => {
  const runtime = (await import(pathToFileURL(join(import.meta.dirname, "../dist/runtime/read.mjs")))).default;
  assert.equal(runtime.description, undefined);
  assert.match(runtime.contractDigest, /^[0-9a-f]{64}$/u);
});

test("executes a Tool only when an Environment invokes its handler", async () => {
  const runtime = (await import(pathToFileURL(join(import.meta.dirname, "../dist/runtime/read.mjs")))).default;
  const workspace = await mkdtemp(join(tmpdir(), "aex-tools-"));
  try {
    await writeFile(join(workspace, "hello.txt"), "hello");
    const output = await runtime.execute(
      { path: "hello.txt" },
      { workspace, deadlineMs: Date.now() + 1_000, signal: new AbortController().signal },
    );
    assert.equal(output.content, "hello");
  } finally {
    await rm(workspace, { recursive: true, force: true });
  }
});

test("rejects a path outside the Environment workspace", async () => {
  const runtime = (await import(pathToFileURL(join(import.meta.dirname, "../dist/runtime/read.mjs")))).default;
  await assert.rejects(
    runtime.execute(
      { path: "../outside" },
      { workspace: tmpdir(), deadlineMs: Date.now() + 1_000, signal: new AbortController().signal },
    ),
    /path escapes/u,
  );
});
