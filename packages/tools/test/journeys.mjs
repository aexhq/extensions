import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { environment } from "@aexhq/brain";
import * as tools from "../dist/index.js";
import { codex } from "../../loop-codex/dist/index.mjs";
import { answer, calls, collect, fixture } from "../../../shared/journey-fixture.mjs";

async function workspace(t) {
  const directory = await mkdtemp(join(tmpdir(), "tools-journey-"));
  const executed = [];
  const server = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const command = JSON.parse(Buffer.concat(chunks).toString());
    const { sequence, request: operation } = command.operation;
    let receipt;
    try {
      assert.equal(command.contract, "environment/v1");
      if (operation.type === "execute") {
        const { name } = operation.implementation;
        assert.deepEqual(operation.implementation, { type: "aex_official_tool", version: 1, name });
        assert.ok(Object.hasOwn(tools, name));
        executed.push(name);
        const runtime = (await import(`../dist/runtime/${name}.mjs`)).default;
        receipt = { type: "result", output: await runtime.execute(operation.input, { workspace: directory, signal: AbortSignal.timeout(10_000) }) };
      } else {
        assert.ok(["setup", "detach", "teardown"].includes(operation.type));
        receipt = { type: "result", output: {} };
      }
    } catch (error) {
      receipt = { type: "failure", code: "tool_error", message: error.message, retryable: false };
    }
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify({ contract: "environment/v1", sequence, receipt }));
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  t.after(async () => { await new Promise(resolve => server.close(resolve)); await rm(directory, { recursive: true, force: true }); });
  const env = environment({ url: () => `http://127.0.0.1:${server.address().port}` })({ name: "workspace" });
  return { directory, executed, tools: Object.values(tools).map(factory => factory({ env })) };
}

test("Tools: create, inspect, edit, search, verify, and track work through the public SDK", { timeout: 30_000 }, async (t) => {
  const f = await fixture(t, [
    () => calls(
      ["write", { path: "notes/task.txt", content: "colour: blue\n" }],
      ["read", { path: "notes/task.txt", offset: 0, limit: 262144 }],
      ["edit", { path: "notes/task.txt", old_text: "blue", new_text: "cyan" }],
      ["ls", { path: "notes", limit: 1000 }], ["glob", { pattern: "**/*.txt", limit: 1000 }],
      ["grep", { pattern: "cyan", path: "notes", limit: 1000 }],
      ["bash", { command: "cat notes/task.txt" }],
      ["todo", { action: "set", items: [{ text: "change colour", done: true }] }],
      ["todo", { action: "get" }],
    ),
    body => {
      const results = body.messages.filter(message => message.role === "tool").map(message => JSON.parse(message.content));
      assert.equal(results.length, 9);
      assert.equal(results[1].content, "colour: blue\n");
      assert.equal(results[2].replacements, 1);
      assert.deepEqual(results[3].entries, [{ name: "task.txt", kind: "file" }]);
      assert.deepEqual(results[4].paths, ["notes/task.txt"]);
      assert.match(results[5].matches[0], /task.txt:1:colour: cyan/u);
      assert.equal(results[6].stdout, "colour: cyan\n");
      assert.deepEqual(results[8].items, [{ text: "change colour", done: true }]);
      return answer("changed and verified cyan");
    },
  ]);
  const w = await workspace(t);
  const session = await f.session(codex, { tools: w.tools });
  await session.send("Change the colour and verify the result");
  assert.equal(await readFile(join(w.directory, "notes/task.txt"), "utf8"), "colour: cyan\n");
  assert.deepEqual(w.executed, ["write", "read", "edit", "ls", "glob", "grep", "bash", "todo", "todo"]);
  assert.equal((await session.transcript()).messages.at(-1).content[0].text, "changed and verified cyan");
  assert.equal((await collect(session.events())).filter(event => event.type === "tool_call_started").length, 9);
});

test("Tools: a missing-file error lets the model choose the next operation", { timeout: 30_000 }, async (t) => {
  const f = await fixture(t, [
    () => calls(["read", { path: "new.txt", offset: 0, limit: 262144 }]),
    body => {
      const content = body.messages.at(-1).content;
      assert.match(content, /^ERROR: /u);
      const result = JSON.parse(content.slice("ERROR: ".length));
      assert.equal(result.code, "tool_error");
      assert.match(result.message, /ENOENT/u);
      assert.equal(result.retryable, false);
      return calls(["write", { path: "new.txt", content: "created by explicit model choice" }]);
    },
    () => answer("created the missing file"),
  ]);
  const w = await workspace(t);
  const session = await f.session(codex, { tools: w.tools });
  await session.send("Read the file, creating it if needed");
  assert.deepEqual(w.executed, ["read", "write"]);
  assert.equal(await readFile(join(w.directory, "new.txt"), "utf8"), "created by explicit model choice");
});
