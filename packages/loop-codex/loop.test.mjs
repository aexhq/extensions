import assert from "node:assert/strict";
import test from "node:test";

import { runTurn } from "@aexhq/brain";

import { codex } from "./src/index.mjs";

// A fake Brain: model answers come off a script, dispatches are recorded and
// answered from a table, appends are recorded.
const host = (responses, { results = {} } = {}) => {
  const record = { requests: [], dispatches: [], appended: [] };
  return {
    record,
    model(requestJson) {
      record.requests.push(JSON.parse(requestJson));
      const response = responses.shift();
      assert.ok(response, "the loop called the model more often than the script allows");
      return JSON.stringify(response);
    },
    dispatch(callsJson) {
      const calls = JSON.parse(callsJson);
      record.dispatches.push(calls.map((call) => call.call_id));
      return JSON.stringify(calls.map((call) => ({ call_id: call.call_id, output: results[call.call_id] ?? "", is_error: false })));
    },
    append(kind, payloadJson) {
      record.appended.push({ kind, payload: JSON.parse(payloadJson) });
      return record.appended.length;
    },
    telemetry() {},
  };
};

const turn = (message, fake, { transcript = [], slots = {}, configuration = {} } = {}) =>
  runTurn(codex, {
    input: { message },
    transcript,
    slots,
    events: [],
    configuration,
    system: "",
    tools: [],
    runtime: { logicalTimeMs: 1n },
  }, fake);

const assistant = (content, usage = {}, stop_reason = "tool_use") => ({
  message: { role: "assistant", content },
  stop_reason,
  usage,
});

test("executes tool calls one at a time and feeds all outputs back in call order", async () => {
  const fake = host(
    [
      assistant([
        { type: "tool_use", id: "c1", name: "bash", input: { command: "make" } },
        { type: "tool_use", id: "c2", name: "read", input: { path: "log" } },
      ]),
      assistant([{ type: "text", text: "built" }], {}, "end_turn"),
    ],
    { results: { c1: "ok", c2: "clean" } },
  );
  await turn("build it", fake);

  assert.deepEqual(fake.record.dispatches, [["c1"], ["c2"]]);
  const results = fake.record.requests[1].messages.at(-1);
  assert.deepEqual(results.content.map((block) => [block.tool_use_id, block.content]), [["c1", "ok"], ["c2", "clean"]]);
});

test("compacts at the 90% token threshold using reported usage, keeping user messages plus a bridge", async () => {
  const fake = host(
    [
      // The provider reports 950 tokens used: past 90% of 1000.
      assistant([{ type: "tool_use", id: "c1", name: "bash", input: {} }], { input_tokens: 900, output_tokens: 50 }),
      assistant([{ type: "text", text: "progress so far" }], {}, "end_turn"),
      assistant([{ type: "text", text: "continuing" }], {}, "end_turn"),
    ],
    { results: { c1: "big output" } },
  );
  const output = await turn("the original task", fake, { configuration: { contextWindow: 1000 } });

  assert.match(fake.record.requests[1].messages.at(-1).content[0].text, /CONTEXT CHECKPOINT COMPACTION/u);
  const rebuilt = fake.record.requests[2].messages;
  // Prior plain user messages survive; assistant messages and tool results do not.
  assert.deepEqual(rebuilt[0], { role: "user", content: [{ type: "text", text: "the original task" }] });
  assert.equal(rebuilt.length, 2);
  assert.match(rebuilt.at(-1).content[0].text, /^Another language model started to solve this problem/u);
  assert.match(rebuilt.at(-1).content[0].text, /progress so far/u);
  assert.equal(output.slots.usage.lastTokens, 0);
});

test("replies when a response carries no tool calls", async () => {
  const fake = host([assistant([{ type: "text", text: "hi" }], { input_tokens: 10, output_tokens: 2 }, "end_turn")]);
  const output = await turn("hello", fake);

  assert.deepEqual(fake.record.dispatches, []);
  assert.deepEqual(fake.record.appended, [{ kind: "output_emitted", payload: { type: "assistant_message", message: "hi" } }]);
  assert.equal(output.slots.usage.lastTokens, 12);
});
