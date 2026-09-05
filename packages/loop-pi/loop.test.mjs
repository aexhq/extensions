import assert from "node:assert/strict";
import test from "node:test";

import { runPi } from "./src/logic.mjs";

// A fake Brain: model answers come off a script, dispatches are recorded and
// answered from a table, appends are recorded.
const host = (responses, { results = {} } = {}) => {
  const record = { requests: [], dispatches: [], emitted: [] };
  return {
    record,
    events: (after) => ({ events: [], next_cursor: after }),
    model(request) {
      record.requests.push(structuredClone(request));
      const response = responses.shift();
      assert.ok(response, "the loop called the model more often than the script allows");
      return response;
    },
    dispatch(calls) {
      record.dispatches.push(calls.map((call) => call.call_id));
      return results[record.dispatches.length - 1] ?? calls.map((call) => ({ call_id: call.call_id, output: "", is_error: false }));
    },
    emit(kind, payload) {
      record.emitted.push({ kind, payload });
      return record.emitted.length;
    },
    telemetry() {},
  };
};

const turn = (message, fake, { transcript = [], slots = {}, configuration = {} } = {}) =>
  runPi({
    input: { message },
    transcript,
    slots,
    events: [],
    configuration,
    system: "",
    tools: [],
  }, fake);

const assistant = (content, stop_reason = "end_turn", usage = {}) => ({
  message: { role: "assistant", content },
  stop_reason,
  usage,
});

test("issues tool calls as one parallel batch and returns results in source order", async () => {
  const fake = host(
    [
      assistant([
        { type: "text", text: "on it" },
        { type: "tool_use", id: "c1", name: "ls", input: {} },
        { type: "tool_use", id: "c2", name: "read", input: { path: "a" } },
      ], "tool_use"),
      assistant([{ type: "text", text: "done" }]),
    ],
    // Completion order reversed; the transcript must keep assistant source order.
    { results: { 0: [
      { call_id: "c2", output: "contents", is_error: false },
      { call_id: "c1", output: "a b c", is_error: false },
    ] } },
  );
  const output = await turn("list and read", fake);

  assert.deepEqual(fake.record.requests[0].messages.at(-1), { role: "user", content: [{ type: "text", text: "list and read" }] });
  assert.deepEqual(fake.record.dispatches, [["c1", "c2"]]);
  const results = fake.record.requests[1].messages.at(-1);
  assert.equal(results.role, "user");
  assert.deepEqual(results.content.map((block) => [block.tool_use_id, block.content]), [["c1", "a b c"], ["c2", "contents"]]);
  assert.deepEqual(fake.record.emitted, [{ kind: "output_emitted", payload: { type: "assistant_message", message: "done" } }]);
  assert.equal(output.transcript.length, 4);
});

test("fails a truncated tool batch without executing it", async () => {
  const fake = host([
    assistant([{ type: "tool_use", id: "c1", name: "write", input: { partial: true } }], "max_tokens"),
    assistant([{ type: "text", text: "retrying" }]),
  ]);
  await turn("go", fake);

  // Straight back to the model, never to the tools.
  assert.deepEqual(fake.record.dispatches, []);
  const failure = fake.record.requests[1].messages.at(-1);
  assert.equal(failure.content[0].type, "tool_result");
  assert.equal(failure.content[0].is_error, true);
  assert.match(failure.content[0].content, /Re-issue the tool call/u);
});

test("compacts older history into a structured checkpoint and keeps the recent tail", async () => {
  // Tiny budgets so two messages cross the threshold.
  const configuration = { contextWindow: 300, reserveTokens: 100, keepRecentTokens: 40 };
  const first = await turn("old task ".repeat(60), host([assistant([{ type: "text", text: "old answer ".repeat(60) }])]), { configuration });

  const fake = host([
    assistant([{ type: "text", text: "## Goal\nfinish the task" }]),
    assistant([{ type: "text", text: "sure" }]),
  ]);
  const second = await turn("new question", fake, { transcript: first.transcript, slots: first.slots, configuration });

  const prompt = fake.record.requests[0].messages.at(-1).content[0].text;
  assert.match(prompt, /## Goal/u);
  assert.match(prompt, /context checkpoint summary/u);
  const rebuilt = fake.record.requests[1].messages;
  assert.match(rebuilt[0].content[0].text, /^Context checkpoint from earlier in this conversation:/u);
  assert.match(rebuilt[0].content[0].text, /finish the task/u);
  assert.deepEqual(rebuilt.at(-1), { role: "user", content: [{ type: "text", text: "new question" }] });
  assert.equal(second.slots.checkpoint.summary, "## Goal\nfinish the task");
  assert.equal(second.transcript[0].content[0].text.startsWith("Context checkpoint"), true);
});

test("pages runtime failures into context and preserves the observation cursor across turns", async () => {
  const fake = host([assistant([{ type: "text", text: "acknowledged" }])]);
  const pages = [];
  fake.events = (after) => {
    pages.push(after);
    if (after === 0) return { events: [{ sequence: 1, event_type: "turn_failed", data: { code: "interrupted" } }], next_cursor: 1 };
    if (after === 1) return { events: [{ sequence: 2, event_type: "environment_unreachable", data: { environment_id: "env_browser" } }], next_cursor: 2 };
    return { events: [], next_cursor: after };
  };
  const first = await turn("continue", fake);
  assert.deepEqual(pages, [0, 1, 2]);
  assert.match(fake.record.requests[0].messages[0].content[0].text, /interrupted/u);
  assert.match(fake.record.requests[0].messages[1].content[0].text, /env_browser/u);
  assert.deepEqual(fake.record.dispatches, []);
  assert.equal(first.slots.observed_sequence, 2);
  const next = host([assistant([{ type: "text", text: "still here" }])]);
  next.events = (after) => { assert.equal(after, 2); return { events: [], next_cursor: after }; };
  const second = await turn("next", next, first);
  assert.equal(second.transcript.filter((message) => message.content.some((block) => block.text?.includes("Runtime observation"))).length, 2);
});
