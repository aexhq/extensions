import assert from "node:assert/strict";
import test from "node:test";
import { hostEnv } from "@aexhq/brain";
import { mcpTools } from "../dist/index.mjs";
import { pi } from "../../loop-pi/dist/index.mjs";
import { fixture, calls, answer, collect } from "../../../shared/journey-fixture.mjs";
import { mcpFixture } from "./fixture.mjs";
import { eventually } from "../../../shared/environment-test-fixture.mjs";
import { schemaFixtures } from "./schema-fixtures.mjs";

test("MCP bridge: structured success, full error evidence and image output through the public SDK", { timeout: 60_000 }, async t => {
  const f = await fixture(t, [
    () => calls(["lookup", { value: "Ada" }]),
    body => { assert.match(body.input.at(-1).output, /Ada/u); return calls(["failure", {}]); },
    body => { assert.match(body.input.at(-1).output, /permission denied/u); return calls(["protocol_failure", {}]); },
    body => { assert.match(body.input.at(-1).output, /mcp_protocol_error/u); return calls(["image", {}]); },
    body => {
      assert.ok(body.input.flatMap(message => Array.isArray(message.content) ? message.content : Array.isArray(message.output) ? message.output : []).some(block => block.type === "input_image"));
      return answer("evidence inspected");
    },
  ]);
  const mcp = await mcpFixture(t);
  const tools = await mcpTools({ client: mcp.client, env: hostEnv({ name: "app" }), publishMedia: async () => "https://example.com/mcp.png", names: ["lookup", "failure", "protocol_failure", "image"] });
  const session = await f.session(pi, { tools });
  await session.send("look up and inspect evidence");
  const events = await collect(session.events());
  const raw = events.find(event => event.type === "mcp_result" && event.data.tool === "failure");
  assert.equal(raw.data.result.structuredContent.code, "permission_denied");
  assert.equal(raw.origin.kind, "tool");
  const results = (await session.transcript()).messages.flatMap(message => message.content).filter(block => block.type === "tool_result");
  assert.equal(results[1].is_error, true);
  assert.equal(results[1].content.code, "mcp_tool_error");
  assert.deepEqual(results[1].content.details, raw.data.result);
  assert.equal(results[2].is_error, true);
  assert.deepEqual(results[2].content.details, { code: -32001, data: { resource: "fixture" } });
  assert.equal(events.filter(event => event.type === "tool_call_started").length, 4);
  assert.equal((await mcp.records()).filter(record => record.type === "call").length, 4);
});

test("MCP bridge: transport loss returns unknown without marking the host unreachable or replaying", { timeout: 45_000 }, async t => {
  const f = await fixture(t, [
    () => calls(["disconnect", {}]),
    body => { assert.match(body.input.at(-1).output, /outcome unknown/u); return answer("external result needs inspection"); },
  ]);
  const mcp = await mcpFixture(t);
  const session = await f.session(pi, { tools: await mcpTools({ client: mcp.client, env: hostEnv({ name: "app" }), names: ["disconnect"] }) });
  await session.send("make one call");
  const events = await collect(session.events());
  assert.equal(events.find(event => event.type === "mcp_failure").data.outcome, "unknown");
  assert.equal((await mcp.records()).filter(record => record.type === "mutation").length, 1);
  const block = (await session.transcript()).messages.flatMap(message => message.content).find(block => block.type === "tool_result");
  assert.equal(block.is_error, true);
  assert.equal(block.content.code, "unknown");
  assert.equal(events.some(event => event.type === "environment_unreachable"), false);
});

test("MCP bridge: Brain cancellation reaches a real server without replay", { timeout: 45_000 }, async t => {
  const f = await fixture(t, [() => calls(["wait", {}])]);
  const mcp = await mcpFixture(t);
  const session = await f.session(pi, { tools: await mcpTools({ client: mcp.client, env: hostEnv({ name: "app" }), names: ["wait"] }) });
  const running = session.send("wait for cancellation").catch(() => {});
  await eventually(async () => (await mcp.records()).some(record => record.type === "call"));
  await session.interrupt();
  await eventually(async () => (await mcp.records()).some(record => record.type === "cancelled"));
  await running;
  assert.equal((await mcp.records()).filter(record => record.type === "call").length, 1);
  const events = await collect(session.events());
  assert.ok(events.some(event => event.type === "turn_failed"));
  const result = events.find(event => event.type === "tool_call_ended").data.result;
  assert.equal(result.is_error, true);
  assert.equal(result.output.code, "cancelled");
});

test("MCP bridge: original JSON Schemas validate through Brain before any remote effects", { timeout: 60_000 }, async t => {
  const steps = [];
  for (const schema of schemaFixtures) {
    steps.push(() => calls([schema.name, schema.invalid[0]]));
    steps.push(body => { assert.match(body.input.at(-1).output, /invalid_input/u); return calls([schema.name, schema.valid[0]]); });
  }
  steps.push(() => answer("schemas validated"));
  const f = await fixture(t, steps);
  const mcp = await mcpFixture(t);
  const session = await f.session(pi, { tools: await mcpTools({ client: mcp.client, env: hostEnv({ name: "app" }), names: schemaFixtures.map(schema => schema.name) }) });
  await session.send("validate the selected schemas");
  const remoteCalls = (await mcp.records()).filter(record => record.type === "call");
  assert.deepEqual(remoteCalls.map(record => record.input), schemaFixtures.map(schema => schema.valid[0]));
  const events = await collect(session.events());
  assert.equal(events.filter(event => event.type === "tool_call_ended" && event.data.result.output.code === "invalid_input").length, schemaFixtures.length);
});
