import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

const runtime = async (name) => (await import(`../dist/runtime/${name}.mjs`)).default;
async function workspace(t) {
  const directory = await mkdtemp(join(tmpdir(), "tools-runtime-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return { workspace: directory, signal: AbortSignal.timeout(10_000) };
}

for (const [name, input] of Object.entries({
  bash: { command: "" }, edit: { path: "a", old_text: "", new_text: "new" },
  glob: { pattern: "*", limit: 0 }, grep: { pattern: "x", limit: -1 },
  ls: { limit: 10_001 }, read: { path: "a", offset: -1 },
  todo: { action: "set", items: [{ text: "" }] }, write: { path: "", content: "bad" },
})) {
  test(`${name}: the built runtime rejects invalid input before execution`, async (t) => {
    const context = await workspace(t);
    await assert.rejects((await runtime(name)).execute(input, context), error => error.name === "ZodError");
  });
}

test("read/write/edit: UTF-8 bytes, windows, and failed edits preserve the file", async (t) => {
  const context = await workspace(t);
  const write = await runtime("write"), read = await runtime("read"), edit = await runtime("edit");
  assert.deepEqual(await write.execute({ path: "notes/a.txt", content: "café\nhello hello" }, context), { path: "notes/a.txt", bytes: 17 });
  assert.deepEqual(await read.execute({ path: "notes/a.txt", offset: 6, limit: 5 }, context), { content: "hello", bytes: 5, truncated: true });
  for (const [old_text, pattern] of [["missing", /not found/u], ["hello", /more than once/u]]) {
    await assert.rejects(edit.execute({ path: "notes/a.txt", old_text, new_text: "bad" }, context), pattern);
    assert.equal((await read.execute({ path: "notes/a.txt" }, context)).content, "café\nhello hello");
  }
  await edit.execute({ path: "notes/a.txt", old_text: "café", new_text: "tea" }, context);
  assert.equal((await read.execute({ path: "notes/a.txt" }, context)).content, "tea\nhello hello");
  await writeFile(join(context.workspace, "binary"), Buffer.from([0, 1]));
  await assert.rejects(read.execute({ path: "binary" }, context), /binary/u);
  await assert.rejects(read.execute({ path: "absent" }, context), /ENOENT/u);
});

test("filesystem Tools refuse paths outside their workspace", async (t) => {
  const context = await workspace(t);
  for (const [name, input] of Object.entries({ read: {}, write: { content: "bad" }, edit: { old_text: "a", new_text: "b" }, ls: {}, grep: { pattern: "x" } })) {
    await assert.rejects((await runtime(name)).execute({ ...input, path: "../outside" }, context), /outside the Environment workspace/u, name);
  }
});

test("ls/glob/grep: results are bounded and report truncation and no matches", async (t) => {
  const context = await workspace(t);
  const write = await runtime("write");
  for (const path of ["b.txt", "a.txt", "nested/c.txt"]) await write.execute({ path, content: "needle\n" }, context);
  assert.deepEqual(await (await runtime("ls")).execute({ limit: 1 }, context), { entries: [{ name: "a.txt", kind: "file" }], truncated: true });
  assert.deepEqual(await (await runtime("glob")).execute({ pattern: "**/*.txt", limit: 1 }, context), { paths: ["a.txt"], truncated: true });
  assert.deepEqual(await (await runtime("glob")).execute({ pattern: "*.txt" }, context), { paths: ["a.txt", "b.txt"], truncated: false });
  const grep = await runtime("grep");
  const found = await grep.execute({ pattern: "needle", limit: 1 }, context);
  assert.equal(found.matches.length, 1);
  assert.match(found.matches[0], /\.txt:1:needle$/u);
  assert.equal(found.truncated, true);
  assert.deepEqual(await grep.execute({ pattern: "absent" }, context), { matches: [], truncated: false });
  await assert.rejects(grep.execute({ pattern: "(" }, context), /regex|parse|unclosed/iu);
});

test("todo: state persists between runtime calls and stays within the selected workspace", async (t) => {
  const first = await workspace(t), second = await workspace(t);
  const todo = await runtime("todo");
  assert.deepEqual(await todo.execute({ action: "get" }, first), { items: [] });
  assert.deepEqual(await todo.execute({ action: "set", items: [{ text: "ship" }] }, first), { items: [{ text: "ship", done: false }] });
  assert.deepEqual(await todo.execute({ action: "get" }, first), { items: [{ text: "ship", done: false }] });
  assert.deepEqual(await todo.execute({ action: "get" }, second), { items: [] });
  await writeFile(join(first.workspace, ".aex/todo.json"), "broken JSON");
  await assert.rejects(todo.execute({ action: "get" }, first), SyntaxError);
});

test("bash: runs in the workspace and preserves failure output", async (t) => {
  const context = await workspace(t);
  const bash = await runtime("bash");
  assert.deepEqual(await bash.execute({ command: "printf saved > result.txt; printf out; printf err >&2; exit 7" }, context), { exit_code: 7, stdout: "out", stderr: "err" });
  assert.equal(await readFile(join(context.workspace, "result.txt"), "utf8"), "saved");
});

test("bash: reports an aborted invocation", async (t) => {
  const context = await workspace(t);
  await assert.rejects((await runtime("bash")).execute({ command: "sleep 30" }, { ...context, signal: AbortSignal.abort() }), /abort/iu);
});
