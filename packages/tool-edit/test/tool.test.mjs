import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { edit as factory } from "../dist/index.js";
import edit from "../dist/runtime/edit.mjs";
import { toolTests, workspace } from "../../../shared/tool-test-fixture.mjs";

toolTests("edit", factory, edit, { path: "a", old_text: "", new_text: "new" });

test("edit: replaces one exact occurrence and failed edits preserve the file", async t => {
  const context = await workspace(t);
  const target = join(context.workspace, "a.txt");
  await writeFile(target, "café\nhello hello");
  for (const [old_text, pattern] of [["missing", /not found/u], ["hello", /more than once/u]]) {
    await assert.rejects(edit.execute({ path: "a.txt", old_text, new_text: "bad" }, context), pattern);
    assert.equal(await readFile(target, "utf8"), "café\nhello hello");
  }
  assert.deepEqual(await edit.execute({ path: "a.txt", old_text: "café", new_text: "tea" }, context), { path: "a.txt", replacements: 1 });
  assert.equal(await readFile(target, "utf8"), "tea\nhello hello");
});

test("edit: refuses paths outside the workspace", async t => {
  await assert.rejects(edit.execute({ path: "../outside", old_text: "a", new_text: "b" }, await workspace(t)), /outside the Environment workspace/u);
});
