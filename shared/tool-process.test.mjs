import assert from "node:assert/strict";
import test from "node:test";
import { PassThrough } from "node:stream";
import { toolProcess } from "./tool-process.mjs";

const operation = { session_id: "session", environment: "workspace", sequence: 3,
  request: { implementation: {}, input: {}, callback: { url: "https://brain.example/call", token: "secret", methods: ["model", "finish"] } } };

test("the child gets invocation services without credentials and concurrent requests keep their identity", async () => {
  const stdout = new PassThrough();
  const writes = [];
  let release;
  const first = new Promise(resolve => { release = resolve; });
  const bridge = toolProcess(operation, { stdout, write: async text => {
    const value = JSON.parse(text); writes.push(value);
    if (value.id === 2) release();
    if (value.id === 1) stdout.write(JSON.stringify({ type: "service", id: 3, method: "finish", input: { status: "ok", value: "done" } }) + "\n");
    if (value.id === 3) stdout.end('{"type":"returned"}\n');
  } }, { fetch: async (_url, request) => {
    assert.equal(request.headers.authorization, "Bearer secret");
    const { input } = JSON.parse(request.body);
    if (input === "one") await first;
    return Response.json(input);
  } });
  stdout.write('{"type":"service","id":1,"method":"model","input":"one"}\n');
  stdout.write('{"type":"service","id":2,"method":"model","input":"two"}\n');
  assert.deepEqual(await bridge, { type: "returned" });
  assert.ok(!JSON.stringify(writes[0]).includes("secret"));
  assert.deepEqual(writes.slice(1).map(value => value.id), [2, 1, 3]);
});

test("missing service grants and uncertain completion cannot claim a committed finish", async () => {
  const stdout = new PassThrough();
  const writes = [];
  const result = toolProcess(operation, { stdout, write: async text => { writes.push(JSON.parse(text)); } }, {
    fetch: async () => { throw new Error("lost acknowledgment"); },
  });
  stdout.end('{"type":"service","id":1,"method":"emit","input":{}}\n{"type":"service","id":2,"method":"finish","input":null}\n{"type":"returned"}\n');
  assert.equal((await result).type, "unknown");
  assert.match(writes.find(value => value.id === 1).error, /not granted/u);
  assert.match(writes.find(value => value.id === 2).error, /lost acknowledgment/u);
});
