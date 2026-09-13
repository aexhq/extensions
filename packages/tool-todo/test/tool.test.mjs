import assert from "node:assert/strict";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { todo as factory } from "../dist/index.js";
import todo from "../dist/runtime/todo.mjs";
import { toolTests, workspace } from "../../../shared/tool-test-fixture.mjs";

toolTests("todo", factory, todo, { action: "set", items: [{ text: "" }] });

test("todo: state persists between runtime calls and stays within the selected workspace", async (t) => {
  const first = await workspace(t), second = await workspace(t);
  assert.deepEqual(await todo.execute({ action: "get" }, first), { items: [] });
  assert.deepEqual(await todo.execute({ action: "set", items: [{ text: "ship" }] }, first), { items: [{ text: "ship", done: false }] });
  assert.deepEqual(await todo.execute({ action: "get" }, first), { items: [{ text: "ship", done: false }] });
  assert.deepEqual(await todo.execute({ action: "get" }, second), { items: [] });
  await writeFile(join(first.workspace, ".aex/todo.json"), "broken JSON");
  await assert.rejects(todo.execute({ action: "get" }, first), SyntaxError);
});

test("todo: a competing write fails explicitly without changing either file", async (t) => {
  const context = await workspace(t);
  const original = { items: [{ text: "original", done: false }] };
  await todo.execute({ action: "set", ...original }, context);
  const pending = join(context.workspace, ".aex/todo.pending");
  await writeFile(pending, "another writer owns this file", { flag: "wx" });
  await assert.rejects(todo.execute({ action: "set", items: [{ text: "replacement" }] }, context), /Todo write conflict/u);
  assert.deepEqual(await todo.execute({ action: "get" }, context), original);
  assert.equal(await readFile(pending, "utf8"), "another writer owns this file");
  await rm(pending);
  const replacement = { items: [{ text: "explicit retry", done: false }] };
  assert.deepEqual(await todo.execute({ action: "set", ...replacement }, context), replacement);
});

test("todo: failed replacement releases its pending write without retrying", async (t) => {
  const context = await workspace(t);
  const target = join(context.workspace, ".aex/todo.json");
  await mkdir(target, { recursive: true });
  await assert.rejects(todo.execute({ action: "set", items: [{ text: "blocked" }] }, context), /EISDIR|EPERM|EACCES/u);
  await assert.rejects(readFile(join(context.workspace, ".aex/todo.pending")), { code: "ENOENT" });
  await rm(target, { recursive: true });
  assert.deepEqual(await todo.execute({ action: "set", items: [{ text: "explicit retry" }] }, context), {
    items: [{ text: "explicit retry", done: false }],
  });
});
