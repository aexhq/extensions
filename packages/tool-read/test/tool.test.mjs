import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { read as factory } from "../dist/index.js";
import read from "../dist/runtime/read.mjs";
import { toolTests, workspace } from "../../../shared/tool-test-fixture.mjs";

toolTests("read", factory, read, { path: "a", offset: -1 });

test("read: UTF-8 bytes, windows, binary files and missing files", async t => {
  const context = await workspace(t);
  await writeFile(join(context.workspace, "a.txt"), "café\nhello hello");
  assert.deepEqual(await read.execute({ path: "a.txt", offset: 6, limit: 5 }, context), { content: "hello", bytes: 5, truncated: true });
  assert.equal((await read.execute({ path: "a.txt" }, context)).content, "café\nhello hello");
  await writeFile(join(context.workspace, "binary"), Buffer.from([0, 1]));
  await assert.rejects(read.execute({ path: "binary" }, context), /binary/u);
  await assert.rejects(read.execute({ path: "absent" }, context), /ENOENT/u);
});

test("read: refuses paths outside the workspace", async t => {
  await assert.rejects(read.execute({ path: "../outside" }, await workspace(t)), /outside the Environment workspace/u);
});
