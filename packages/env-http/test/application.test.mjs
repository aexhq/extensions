import test from "node:test";
import assert from "node:assert/strict";
import { inspectTool, tool } from "@aexhq/brain";
import { z } from "zod";
import { application, applicationTool } from "../dist/index.mjs";
import { createToolHandler } from "../dist/handler.mjs";
import { createHttpEnvironment, serveEnvironment } from "../dist/server.mjs";

const env = application({ name: "app", url: "https://bridge.example", endpoint: "https://app.example/tools", credential: "application-secret" });
const operation = (request, sequence = 3) => ({ contract: "environment/v1", operation: { session_id: "session", environment: "app", sequence, request } });
const authorized = request => assert.equal(request.headers.get("authorization"), "Bearer application-secret");

function fixture(factory, { options = {}, afterReply, beforeAck } = {}) {
  const placed = factory(options);
  const implementation = inspectTool(applicationTool(factory({ ...options, env }), { env })).implementation;
  const calls = [];
  let relay;
  let dispatched = 0;
  const handler = createToolHandler({ tools: [placed], authorize: authorized,
    callbackRequest: async (url, { token, body }) => {
      assert.equal(url, "https://bridge.example/callback");
      return bridge.callback({ authorization: `Bearer ${token}` }, body);
    } });
  const bridge = createHttpEnvironment({
    authorize: async () => ({ url: "https://app.example/tools", token: "application-secret", timeoutMs: 1000,
      tools: [inspectTool(placed).definition], callbackUrl: "https://bridge.example/callback" }),
    request: async (url, { token, body, signal }) => {
      dispatched++;
      relay = body.callback;
      assert.equal(JSON.stringify(body).includes("private-brain-grant"), false);
      const response = await handler(new Request(url, { method: "POST", headers: { authorization: `Bearer ${token}` }, body: JSON.stringify(body), signal }));
      const value = await response.json();
      await afterReply?.(value);
      return value;
    },
    fetch: async (url, request) => {
      assert.equal(url, "http://brain/callback");
      assert.equal(request.headers.authorization, "Bearer private-brain-grant");
      calls.push(JSON.parse(request.body));
      await beforeAck?.(calls.at(-1));
      return Response.json(calls.length + 3);
    },
  });
  const execute = (override = {}) => bridge.handle(operation({ type: "execute", implementation, input: {}, deadline_ms: 1000,
    callback: { url: "http://brain/callback", token: "private-brain-grant", methods: ["result", "returned", "finish"] }, ...override }));
  return { bridge, execute, calls, relay: () => relay, dispatched: () => dispatched };
}

test("ordinary configured Tools retain parsed options, content and durable completion acknowledgment", async () => {
  let acknowledge, entered;
  const ack = new Promise(resolve => { acknowledge = resolve; });
  const pending = new Promise(resolve => { entered = resolve; });
  let afterFinish = false;
  const lookup = tool({ name: "lookup", description: "Lookup", input: z.object({}),
    options: z.object({ prefix: z.string().transform(value => value.toUpperCase()) }),
    run: async (_, ctx) => { await ctx.finish({ value: ctx.options.prefix }, { content: "Found" }); afterFinish = true; } });
  const f = fixture(lookup, { options: { prefix: "lower" }, beforeAck: async () => { entered(); await ack; } });
  const running = f.execute();
  await pending;
  assert.equal(afterFinish, false);
  acknowledge();
  assert.equal((await running).receipt.type, "accepted");
  assert.equal(afterFinish, true);
  assert.deepEqual(f.calls, [{ method: "finish", input: { status: "ok", value: { value: "LOWER" }, content: "Found" } }]);
  await assert.rejects(f.bridge.callback({ authorization: `Bearer ${f.relay().token}` }, { method: "finish", input: null }), /not available/);
});

test("recorded completion survives loss of the final application reply without a second finish", async () => {
  const read = tool({ name: "read", description: "Read", input: z.object({}), run: (_, ctx) => ctx.finish() });
  const f = fixture(read, { afterReply: () => { throw new Error("lost reply"); } });
  assert.equal((await f.execute()).receipt.type, "accepted");
  assert.deepEqual(f.calls, [{ method: "finish", input: null }]);
  assert.equal(f.dispatched(), 1);
});

test("lost completion acknowledgment is uncertain and is never repeated", async () => {
  const read = tool({ name: "read", description: "Read", input: z.object({}), run: (_, ctx) => ctx.finish("value") });
  const f = fixture(read, { beforeAck: () => { throw new Error("lost acknowledgment"); } });
  assert.notEqual((await f.execute()).receipt.type, "accepted");
  assert.equal(f.calls.length, 1);
  assert.equal(f.dispatched(), 1);
});

test("changed configured options fail before business code", async () => {
  let executed = false;
  const read = tool({ name: "read", description: "Read", input: z.object({}), options: z.object({ region: z.string() }),
    run: (_, ctx) => { executed = true; return ctx.finish("value"); } });
  const f = fixture(read, { options: { region: "west" } });
  const implementation = inspectTool(applicationTool(read({ env, region: "east" }), { env })).implementation;
  assert.equal((await f.execute({ implementation })).receipt.code, "incompatible_tool");
  assert.equal(executed, false);
  assert.equal(f.calls.length, 0);
});

test("shared server passes authenticated context separately and rejects unknown callback capabilities", async () => {
  const f = fixture(tool({ name: "read", description: "Read", input: z.object({}), run: (_, ctx) => ctx.finish() }));
  const server = await serveEnvironment(async (body, context) => { assert.deepEqual(context, { private: true }); return body; }, {
    authenticate: headers => { if (headers.authorization !== "Bearer context-secret") throw new Error("private credential"); return { private: true }; },
    callback: f.bridge.callback,
  });
  try {
    const denied = await fetch(`${server.url}/v1/operations`, { method: "POST", body: "{}" });
    assert.equal(denied.status, 401);
    assert.equal((await denied.text()).includes("private credential"), false);
    const allowed = await fetch(`${server.url}/v1/operations`, { method: "POST", headers: { authorization: "Bearer context-secret" }, body: '{"ok":true}' });
    assert.deepEqual(await allowed.json(), { ok: true });
    assert.equal((await fetch(`${server.url}/v1/callback`, { method: "POST", body: '{"method":"finish","input":null}' })).status, 403);
  } finally { await server.close(); }
});
