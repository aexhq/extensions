import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { write as factory } from "../dist/index.js";
import write from "../dist/runtime/write.mjs";
import { toolTests, workspace } from "../../../shared/tool-test-fixture.mjs";

toolTests("write", factory, write, { path: "", content: "bad" });

test("write: creates parent directories and reports UTF-8 bytes", async t => {
  const context = await workspace(t);
  assert.deepEqual(await write.execute({ path: "notes/a.txt", content: "café\nhello hello" }, context), { path: "notes/a.txt", bytes: 17 });
  assert.equal(await readFile(join(context.workspace, "notes/a.txt"), "utf8"), "café\nhello hello");
});

test("write: refuses paths outside the workspace", async t => {
  await assert.rejects(write.execute({ path: "../outside", content: "bad" }, await workspace(t)), /outside the Environment workspace/u);
});
