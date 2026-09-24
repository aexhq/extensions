import test from "node:test";
import assert from "node:assert/strict";
import { tool, inspectTool } from "@aexhq/brain";
import { z } from "zod";
import { http, httpTool } from "../dist/index.mjs";
import { createToolHandler } from "../dist/handler.mjs";
import { createHttpEnvironment } from "../dist/server.mjs";
import { publicAddress, publicJson } from "../src/outbound.mjs";

const lookup = tool({ name: "lookup", description: "Read a record", input: z.object({ id: z.string().default("A") }),
  output: z.object({ status: z.string() }), run: ({ id }, ctx) => {
    assert.equal(id, "A"); assert.equal(ctx.sessionId, "session"); return { status: "cancelled" };
  } });
const definition = inspectTool(lookup()).definition;
const invocation = { contract: "http-tool/v1", sessionId: "session", environment: "app", sequence: 3,
  tool: definition, input: {}, deadlineAtMs: Date.now() + 60_000 };
const command = (request, sequence = 3) => ({ contract: "environment/v1", operation: { session_id: "session", environment: "app", sequence, request } });
const execute = { type: "execute", implementation: { type: "http_tool", definition }, input: {}, deadline_ms: 5000,
  callback: { url: "http://brain/callback", token: "private-callback", methods: ["finish"] } };
const binding = { url: "https://app.example/tools", token: "private-app", timeoutMs: 1000, tools: [definition] };
const authorized = request => { if (request.headers.get("authorization") !== "Bearer private-app") throw new Error("denied"); };
const requestFor = (body, token = "private-app") => new Request(binding.url, { method: "POST", headers: { authorization: `Bearer ${token}` }, body: JSON.stringify(body) });

test("one Tool contract supplies the placement and fresh request handlers; business statuses remain data", async () => {
  const env = http({ name: "app", url: "https://bridge.example", binding: "app-v1" });
  assert.deepEqual(inspectTool(httpTool(lookup(), { env })).definition, definition);
  let invocations = 0, finishes = 0;
  const bridge = createHttpEnvironment({ authorize: async () => binding,
    request: async (url, { token, body }) => {
      assert.equal(url, binding.url); assert.equal(token, binding.token);
      assert.equal(JSON.stringify(body).includes("private-callback"), false);
      invocations++;
      return createToolHandler({ tools: [lookup()], authorize: authorized })(requestFor(body)).then(r => r.json());
    },
    fetch: async (url, options) => {
      finishes++;
      assert.equal(options.headers.authorization, "Bearer private-callback");
      assert.deepEqual(JSON.parse(options.body), { method: "finish", input: { status: "ok", value: { status: "cancelled" } } });
      return Response.json(4);
    },
  });
  assert.equal((await bridge.handle(command({ type: "setup", configuration: { binding: "app-v1" } }, 1))).receipt.type, "accepted");
  for (const sequence of [3, 8]) assert.equal((await bridge.handle(command(execute, sequence))).receipt.type, "result");
  assert.equal(invocations, 2); assert.equal(finishes, 2);
});

test("authorization and advertised-contract changes fail before business code", async () => {
  let calls = 0;
  const placed = tool({ name: "lookup", description: "Read a record", input: z.object({ id: z.number() }), run: () => { calls++; return {}; } })();
  const handler = createToolHandler({ tools: [placed], authorize: authorized });
  assert.equal((await (await handler(requestFor(invocation, "wrong"))).json()).code, "denied");
  assert.equal((await (await handler(requestFor(invocation))).json()).code, "incompatible_tool");
  assert.equal(calls, 0);
  const bridge = createHttpEnvironment({ authorize: async () => ({ ...binding, tools: [] }), request: () => assert.fail("dispatched unapproved Tool") });
  assert.equal((await bridge.handle(command(execute))).receipt.code, "denied");
});

test("async input/output validation transforms values and rejects invalid output", async () => {
  const factory = tool({ name: "async", description: "Validate", input: z.string().transform(async s => s.toUpperCase()),
    output: z.string().refine(async s => s === "OK"), run: value => value });
  const handler = createToolHandler({ tools: [factory()], authorize: authorized });
  const call = { ...invocation, tool: inspectTool(factory()).definition };
  assert.deepEqual(await (await handler(requestFor({ ...call, input: "ok" }))).json(), { type: "success", value: "OK" });
  assert.equal((await (await handler(requestFor({ ...call, input: "bad" }))).json()).code, "invalid_output");
  assert.equal((await (await handler(requestFor({ ...call, input: null }))).json()).code, "invalid_input");
});

test("lost replies and lost completion acknowledgements stay unknown without another request", async () => {
  for (const lost of ["reply", "malformed", "completion"]) {
    let requests = 0, completions = 0;
    const bridge = createHttpEnvironment({ authorize: async () => binding,
      request: async () => { requests++; if (lost === "reply") throw new Error("lost"); return lost === "malformed" ? {} : { type: "success", value: {} }; },
      fetch: async () => { completions++; throw new Error("lost"); },
    });
    assert.equal((await bridge.handle(command(execute))).receipt.type, "unknown");
    assert.equal(requests, 1); assert.equal(completions, lost === "completion" ? 1 : 0);
  }
});

test("deadlines and cancellation stop the request without promising rollback", async () => {
  for (const cause of ["timeout", "cancelled"]) {
    let entered;
    const started = new Promise(resolve => { entered = resolve; });
    const bridge = createHttpEnvironment({ authorize: async () => ({ ...binding, timeoutMs: cause === "timeout" ? 10 : 1000 }),
      request: (_url, { signal }) => new Promise((_, reject) => {
        entered(); signal.addEventListener("abort", () => reject(new Error("aborted")), { once: true });
      }), fetch: () => assert.fail("interrupted request finished successfully") });
    const pending = bridge.handle(command(execute));
    await started;
    if (cause === "cancelled") await bridge.handle(command({ type: "cancel", target_sequence: 3 }, 4));
    assert.equal((await pending).receipt.code, cause);
  }
});

test("destination resolution denies private, mapped, metadata and transition addresses", async () => {
  for (const address of ["127.0.0.1", "10.0.0.1", "169.254.169.254", "172.16.2.3", "192.168.1.2", "100.64.0.1", "::1", "::ffff:127.0.0.1", "fc00::1", "fe80::1", "2002:7f00:1::1", "2001:db8::1"])
    assert.equal(publicAddress(address), false, address);
  for (const address of ["1.1.1.1", "2606:4700:4700::1111"]) assert.equal(publicAddress(address), true);
  assert.throws(() => publicJson("https://127.0.0.1/tools", { token: "x", body: {} }), /not public/u);
  await assert.rejects(publicJson("https://localhost/tools", { token: "x", body: {}, signal: AbortSignal.timeout(1000) }), /non-public/u);
  for (const url of ["http://app.example", "https://user:secret@app.example", "https://app.example:8080", "https://app.example/?token=secret"])
    assert.throws(() => publicJson(url, { token: "x", body: {} }), /HTTPS endpoint/u);
});
