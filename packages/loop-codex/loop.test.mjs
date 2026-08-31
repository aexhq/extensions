import assert from "node:assert/strict";
import test from "node:test";

import { activateAgentloop } from "@aexhq/brain";

import { codex } from "./src/index.mjs";

const drive = (configuration = {}) => {
  let state;
  return (observation) => {
    const output = activateAgentloop(codex, {
      context: { state },
      observation,
      configuration,
      runtime: { logicalTimeMs: 1n },
    });
    state = output.context.state;
    return output.decision;
  };
};

const assistant = (content, usage = {}) => ({
  type: "model_completed",
  response: { message: { role: "assistant", content }, stop_reason: "tool_use", usage },
});

test("executes tool calls one at a time and feeds all outputs back in call order", () => {
  const step = drive();
  step({ type: "user_message", content: "build it" });
  const first = step(assistant([
    { type: "tool_use", id: "c1", name: "bash", input: { command: "make" } },
    { type: "tool_use", id: "c2", name: "read", input: { path: "log" } },
  ]));
  assert.equal(first.type, "tools");
  assert.deepEqual(first.calls.map((call) => call.callId), ["c1"]);

  const second = step({ type: "tools_completed", results: [{ call_id: "c1", output: "ok", is_error: false }] });
  assert.equal(second.type, "tools");
  assert.deepEqual(second.calls.map((call) => call.callId), ["c2"]);

  const next = step({ type: "tools_completed", results: [{ call_id: "c2", output: "clean", is_error: false }] });
  assert.equal(next.type, "model");
  const results = next.request.messages.at(-1);
  assert.deepEqual(results.content.map((block) => [block.tool_use_id, block.content]), [["c1", "ok"], ["c2", "clean"]]);
});

test("compacts at the 90% token threshold using reported usage, keeping user messages plus a bridge", () => {
  const step = drive({ contextWindow: 1000 });
  step({ type: "user_message", content: "the original task" });
  // The provider reports 950 tokens used: past 90% of 1000.
  const calls = step(assistant([
    { type: "tool_use", id: "c1", name: "bash", input: {} },
  ], { input_tokens: 900, output_tokens: 50 }));
  assert.equal(calls.type, "tools");

  const compaction = step({ type: "tools_completed", results: [{ call_id: "c1", output: "big output", is_error: false }] });
  assert.equal(compaction.type, "model");
  assert.match(compaction.request.messages.at(-1).content[0].text, /CONTEXT CHECKPOINT COMPACTION/u);

  const resumed = step({
    type: "model_completed",
    response: { message: { role: "assistant", content: [{ type: "text", text: "progress so far" }] }, stop_reason: "end_turn", usage: {} },
  });
  assert.equal(resumed.type, "model");
  const rebuilt = resumed.request.messages;
  // Prior plain user messages survive; assistant messages and tool results do not.
  assert.deepEqual(rebuilt[0], { role: "user", content: [{ type: "text", text: "the original task" }] });
  assert.equal(rebuilt.length, 2);
  assert.match(rebuilt.at(-1).content[0].text, /^Another language model started to solve this problem/u);
  assert.match(rebuilt.at(-1).content[0].text, /progress so far/u);
});

test("replies when a response carries no tool calls", () => {
  const step = drive();
  step({ type: "user_message", content: "hello" });
  const reply = step({
    type: "model_completed",
    response: { message: { role: "assistant", content: [{ type: "text", text: "hi" }] }, stop_reason: "end_turn", usage: {} },
  });
  assert.equal(reply.type, "emit");
  assert.equal(reply.event.content, "hi");
});
