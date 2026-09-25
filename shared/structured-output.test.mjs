import { finishedResult } from "./loop-test-fixture.mjs";
import test from "node:test";
import assert from "node:assert/strict";
import { runPi } from "../packages/loop-pi/src/logic.mjs";
import { runCodex } from "../packages/loop-codex/src/logic.mjs";

const schema = { type: "object", properties: { answer: { type: "integer" } }, required: ["answer"], additionalProperties: false };
const answer = (text, stop_reason = "end_turn") => ({ message: { role: "assistant", content: [{ type: "text", text }] }, stop_reason, usage: {} });
const toolCall = () => ({ message: { role: "assistant", content: [{ type: "tool_use", id: "call-1", name: "calculate", input: {} }] }, stop_reason: "tool_use", usage: {} });
const input = output => ({ input: { message: "calculate" }, transcript: [], configuration: { compaction: false, output },
  tools: [{ name: "calculate", description: "calculate", environments: ["sandbox"], input_schema: { type: "object" } }] });
function host(responses) {
  const calls = [];
  const emitted = [];
  const invocations = [];
  const context = { environments: { list: async () => [] }, calls, emitted, invocations, acknowledge() {}, kv: { read() {}, put() {} }, events: after => ({ events: [], next_cursor: after }),
    setTranscript(transcript) { context.transcript = structuredClone(transcript); },
    model(request) { calls.push(structuredClone(request)); const response = responses.shift(); if (response instanceof Error) throw response; assert.ok(response, "unexpected model call"); return response; },
    emit(type, data) { emitted.push({ type, data }); },
    dispatch(request) { invocations.push(request); return [finishedResult({ call_id: "call-1", output: 42, is_error: false })]; } };
  return context;
}
for (const run of [runPi, runCodex]) {
  test(`${run.name}: hosted corrections keep prior effects and only emit a valid answer`, async () => {
    const context = host([toolCall(), answer("not json"), answer('{"answer":"42"}'), answer('{"answer":42}')]);
    assert.deepEqual(await run(input({ schema }), context), { result: { answer: 42 } });
    assert.equal(context.invocations.length, 1);
    assert.equal(context.calls[1].tools.length, 1);
    assert.deepEqual(context.calls.slice(2).map(call => call.tools), [[], []]);
    assert.equal(context.emitted.length, 1);
    assert.equal(context.emitted[0].data.message, '{"answer":42}');
    assert.match(context.calls.at(-1).messages.at(-1).content[0].text, /JSON Schema/u);
    assert.match(JSON.stringify(context.transcript), /Validation feedback/u);
  });
  test(`${run.name}: invalid schemas fail before effects and correction exhaustion is exact`, async () => {
    const invalid = host([]);
    await assert.rejects(run(input({ schema: { type: "object", unknown_constraint: true } }), invalid), /unknown keyword/u);
    await assert.rejects(run(input({ schema: { $ref: "https://example.com/schema" } }), invalid), /resolve reference/u);
    assert.equal(invalid.calls.length, 0);
    const exhausted = host([answer("{}"), answer("{}")]);
    await assert.rejects(run(input({ schema, maxCorrections: 1 }), exhausted), /after 2 attempts/u);
    assert.equal(exhausted.calls.length, 2);
    assert.equal(exhausted.emitted.length, 0);
  });
  for (const stop of ["refusal", "max_tokens", "unknown"]) test(`${run.name}: ${stop} is not an output correction`, async () => {
    const context = host([answer("{}", stop)]);
    await assert.rejects(run(input({ schema }), context), /no correction was attempted/u);
    assert.equal(context.calls.length, 1);
  });
  test(`${run.name}: provider failures and forbidden correction tool calls do not repeat effects`, async () => {
    const failed = host([toolCall(), new Error("connection lost")]);
    await assert.rejects(run(input({ schema }), failed), /connection lost/u);
    assert.equal(failed.invocations.length, 1);
    const forbidden = host([answer("{}"), toolCall()]);
    await assert.rejects(run(input({ schema }), forbidden), /cannot dispatch tools/u);
    assert.equal(forbidden.invocations.length, 0);
  });
}
