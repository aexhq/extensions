import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { ls as factory } from "../dist/index.js";
import ls from "../dist/runtime/ls.mjs";
import { toolTests, workspace } from "../../../shared/tool-test-fixture.mjs";

toolTests("ls", factory, ls, { limit: 10001 });

test("ls: entries are sorted and bounded with truncation", async t => {
  const context = await workspace(t);
  await mkdir(join(context.workspace, "nested"));
  for (const path of ["b.txt", "a.txt"]) await writeFile(join(context.workspace, path), "needle\n");
  assert.deepEqual(await ls.execute({ limit: 1 }, context), { entries: [{ name: "a.txt", kind: "file" }], truncated: true });
  assert.deepEqual(await ls.execute({ }, context), {
    entries: [{ name: "a.txt", kind: "file" }, { name: "b.txt", kind: "file" }, { name: "nested", kind: "dir" }], truncated: false,
  });
});

test("ls: refuses paths outside the workspace", async t => {
  await assert.rejects(ls.execute({ path: "../outside" }, await workspace(t)), /outside the Environment workspace/u);
});
