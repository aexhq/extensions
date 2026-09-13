import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { grep as factory } from "../dist/index.js";
import grep from "../dist/runtime/grep.mjs";
import { toolTests, workspace } from "../../../shared/tool-test-fixture.mjs";

toolTests("grep", factory, grep, { pattern: "x", limit: -1 });

test("grep: matches are bounded and report truncation, no matches and invalid patterns", async t => {
  const context = await workspace(t);
  await mkdir(join(context.workspace, "nested"));
  for (const path of ["b.txt", "a.txt", "nested/c.txt"]) await writeFile(join(context.workspace, path), "needle\n");
  const found = await grep.execute({ pattern: "needle", limit: 1 }, context);
  assert.equal(found.matches.length, 1);
  assert.match(found.matches[0], /\.txt:1:needle$/u);
  assert.equal(found.truncated, true);
  assert.deepEqual(await grep.execute({ pattern: "absent" }, context), { matches: [], truncated: false });
  await assert.rejects(grep.execute({ pattern: "(" }, context), /regex|parse|unclosed/iu);
});

test("grep: refuses paths outside the workspace", async t => {
  await assert.rejects(grep.execute({ path: "../outside", pattern: "x" }, await workspace(t)), /outside the Environment workspace/u);
});
