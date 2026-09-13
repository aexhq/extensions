import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { glob as factory } from "../dist/index.js";
import glob from "../dist/runtime/glob.mjs";
import { toolTests, workspace } from "../../../shared/tool-test-fixture.mjs";

toolTests("glob", factory, glob, { pattern: "*", limit: 0 });

test("glob: matching paths are bounded with truncation", async t => {
  const context = await workspace(t);
  await mkdir(join(context.workspace, "nested"));
  for (const path of ["b.txt", "a.txt", "nested/c.txt"]) await writeFile(join(context.workspace, path), "needle\n");
  assert.deepEqual(await glob.execute({ pattern: "**/*.txt", limit: 1 }, context), { paths: ["a.txt"], truncated: true });
  assert.deepEqual(await glob.execute({ pattern: "*.txt" }, context), { paths: ["a.txt", "b.txt"], truncated: false });
});

test("glob: double-star slash matches directories, not basename prefixes", async t => {
  const context = await workspace(t);
  await mkdir(join(context.workspace, "sub/deep"), { recursive: true });
  for (const path of ["package.json", "notpackage.json", "sub/package.json", "sub/notpackage.json", "sub/deep/package.json"]) {
    await writeFile(join(context.workspace, path), "{ }");
  }
  assert.deepEqual(await glob.execute({ pattern: "**/package.json" }, context), {
    paths: ["package.json", "sub/deep/package.json", "sub/package.json"], truncated: false,
  });
  assert.deepEqual(await glob.execute({ pattern: "sub/**/package.json" }, context), {
    paths: ["sub/deep/package.json", "sub/package.json"], truncated: false,
  });
});
