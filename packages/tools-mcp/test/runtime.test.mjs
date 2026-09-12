import assert from "node:assert/strict";
import test from "node:test";
import { hostEnv, inspectTool } from "@aexhq/brain";
import { mcpTools } from "../dist/index.mjs";
import { mcpFixture } from "./fixture.mjs";
import { eventually } from "../../../shared/environment-test-fixture.mjs";
import { SdkError, SdkErrorCode } from "@modelcontextprotocol/client";
import { schemaFixtures } from "./schema-fixtures.mjs";

function context(signal = new AbortController().signal) {
  const events = [];
  return { events, signal, deadline: new Date(Date.now() + 15_000), sequence: 1,
    emit: async (type, data) => { events.push({ type, data }); return events.length; } };
}

test("MCP publishes PDF bytes before retaining success or error evidence and never repeats a completed tool", async () => {
  for (const isError of [false, true]) {
    let calls = 0;
    const bytes = Buffer.from("%PDF-1.7\nfixture");
    const client = { listTools: async () => ({ tools: [{ name: "report", inputSchema: { type: "object" } }] }), callTool: async () => {
      calls++;
      return { isError, content: [{ type: "resource", resource: { uri: "report:///one", mimeType: "application/pdf", blob: bytes.toString("base64") } }] };
    } };
    for (const publisher of [undefined, async () => { throw new Error("publication failed"); }, async input => {
      assert.deepEqual(Buffer.from(input.bytes), bytes);
      return "https://example.com/report.pdf";
    }]) {
      const [source] = (await mcpTools({ client, env: hostEnv({ name: "app" }), names: ["report"], publishMedia: publisher })).map(inspectTool);
      const call = context();
      const before = calls;
      await source.handler({}, call);
      assert.equal(calls, before + 1);
      assert.equal(JSON.stringify(call.events).includes(bytes.toString("base64")), false);
      assert.ok(["mcp_result", "mcp_media_failed"].includes(call.events[0].type));
      if (call.events[0].type === "mcp_result") assert.equal(call.events[0].data.result.media[0].media_type, "application/pdf");
    }
  }
});

test("real MCP discovery freezes selected schemas and preserves evidence, errors and images", { timeout: 30_000 }, async t => {
  const { client, records } = await mcpFixture(t);
  const env = hostEnv({ name: "app" });
  const placed = await mcpTools({ client, env, prefix: "service_", publishMedia: async ({ bytes, mediaType }) => { assert.ok(bytes.length); assert.equal(mediaType, "image/png"); return "https://example.com/mcp.png"; }, names: ["lookup", "failure", "image", "change", "continuation"] });
  const sources = placed.map(inspectTool);
  assert.deepEqual(sources.map(source => source.definition.name), ["service_lookup", "service_failure", "service_image", "service_change", "service_continuation"]);
  assert.ok((await records()).some(record => record.type === "list" && record.cursor === "second"));
  const [lookup, failure, image, change, continuation] = sources;
  assert.throws(() => lookup.contract.input.parse({ value: "unauthorized" }));
  assert.equal((await records()).some(record => record.type === "call"), false);
  const call = context();
  const output = await lookup.handler({ value: "Ada" }, call);
  assert.deepEqual(output.content.structuredContent, { value: "Ada" });
  assert.equal(call.events[0].type, "mcp_result");
  assert.equal(output.content.evidenceSequence, 1);
  const failed = context();
  const failureOutcome = await failure.handler({}, failed);
  assert.equal(failureOutcome.status, "error");
  assert.equal(failureOutcome.error.code, "mcp_tool_error");
  assert.equal(failureOutcome.error.retryable, false);
  assert.deepEqual(failureOutcome.error.details, failed.events[0].data.result);
  assert.equal(failed.events[0].data.result.structuredContent.code, "permission_denied");
  const media = await image.handler({}, context());
  assert.equal(media.media[0].url, "https://example.com/mcp.png");
  assert.equal(media.content.content.length, 0);
  await change.handler({}, context());
  assert.ok((await client.listTools(undefined, { cacheMode: "refresh" })).tools.some(tool => tool.name === "added"));
  assert.equal(sources.some(source => source.definition.name === "service_added"), false);
  assert.equal((await lookup.handler({ value: "Lin" }, context())).content.structuredContent.value, "Lin");
  assert.equal((await continuation.handler({}, context())).error.code, "mcp_unsupported_result");
  assert.equal((await records()).filter(record => record.type === "call" && record.name === "continuation").length, 1);
  await assert.rejects(mcpTools({ client, env, names: ["missing"] }), /expected one MCP definition/u);
});

test("MCP cancellation reaches the server and a lost result is never replayed", { timeout: 30_000 }, async t => {
  const { client, records } = await mcpFixture(t);
  const [wait, disconnect] = (await mcpTools({ client, env: hostEnv({ name: "app" }), names: ["wait", "disconnect"] })).map(inspectTool);
  const controller = new AbortController();
  const running = wait.handler({}, context(controller.signal));
  const cancelled = assert.rejects(running);
  await eventually(async () => (await records()).some(record => record.type === "call" && record.name === "wait"));
  controller.abort(new Error("fixture cancelled"));
  await cancelled;
  await eventually(async () => (await records()).some(record => record.type === "cancelled"));
  const uncertain = context();
  assert.equal((await disconnect.handler({}, uncertain)).status, "unknown");
  assert.equal(uncertain.events[0].data.outcome, "unknown");
  assert.equal((await records()).filter(record => record.type === "mutation").length, 1);
});

test("MCP JSON Schemas survive discovery, validate without coercion and reject unresolved references", async t => {
  const { client, records } = await mcpFixture(t);
  const env = hostEnv({ name: "app" });
  const sources = (await mcpTools({ client, env, names: schemaFixtures.map(fixture => fixture.name) })).map(inspectTool);
  for (const [index, fixture] of schemaFixtures.entries()) {
    const source = sources[index];
    assert.deepEqual(source.definition.inputSchema, fixture.schema);
    for (const input of fixture.invalid) assert.throws(() => source.contract.input.parse(input));
    for (const input of fixture.valid) {
      const parsed = source.contract.input.parse(input);
      assert.deepEqual(parsed, input);
      assert.deepEqual((await source.handler(parsed, context())).content.structuredContent, input);
    }
  }
  assert.equal((await records()).filter(record => record.type === "call").length, schemaFixtures.flatMap(fixture => fixture.valid).length);
  await assert.rejects(mcpTools({ client, env, names: ["unresolved"] }), /ref|resolve/iu);
});

test("MCP protocol failures retain their numeric code and data", async t => {
  const { client } = await mcpFixture(t);
  const [source] = (await mcpTools({ client, env: hostEnv({ name: "app" }), names: ["protocol_failure"] })).map(inspectTool);
  const outcome = await source.handler({}, context());
  assert.equal(outcome.error.code, "mcp_protocol_error");
  assert.deepEqual(outcome.error.details, { code: -32001, data: { resource: "fixture" } });
});

test("MCP SDK timeouts and known local failures have specific terminal outcomes", async () => {
  for (const [code, status] of [[SdkErrorCode.RequestTimeout, "timeout"], [SdkErrorCode.NotConnected, "error"], [SdkErrorCode.InvalidResult, "error"], [SdkErrorCode.SendFailed, "unknown"]]) {
    const client = { listTools: async () => ({ tools: [{ name: "test", inputSchema: { type: "object" } }] }),
      callTool: async () => { throw new SdkError(code, "fixture failure", { cause: "fixture" }); } };
    const [source] = (await mcpTools({ client, env: hostEnv({ name: "app" }), names: ["test"] })).map(inspectTool);
    const call = context();
    const outcome = await source.handler({}, call);
    assert.equal(outcome.status, status);
    if (status === "error") assert.deepEqual(outcome.error.details, { code, data: { cause: "fixture" } });
    assert.equal(call.events[0].data.code, code);
  }
});

test("a real MCP request deadline returns timeout and cancels the remote wait", async t => {
  const { client, records } = await mcpFixture(t);
  const [source] = (await mcpTools({ client, env: hostEnv({ name: "app" }), names: ["wait"] })).map(inspectTool);
  const call = { ...context(), deadline: new Date(Date.now() + 100) };
  assert.deepEqual(await source.handler({}, call), { status: "timeout" });
  await eventually(async () => (await records()).some(record => record.type === "cancelled"));
  assert.equal((await records()).filter(record => record.type === "call").length, 1);
});
