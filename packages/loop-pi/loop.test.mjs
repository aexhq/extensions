import assert from "node:assert/strict";
import test from "node:test";

import { activateAgentloop } from "@aexhq/brain";

import { pi } from "./src/index.mjs";

const drive = (configuration = {}) => {
  let state;
  return (observation) => {
    const output = activateAgentloop(pi, {
      context: { state },
      observation,
      configuration,
      runtime: { logicalTimeMs: 1n },
    });
    state = output.context.state;
    return output.decision;
  };
};

const assistant = (content, stop_reason = "end_turn", usage = {}) => ({
  type: "model_completed",
  response: { message: { role: "assistant", content }, stop_reason, usage },
});

test("issues tool calls as one parallel batch and returns results in source order", () => {
  const step = drive();
  const first = step({ type: "user_message", input: { message: "list and read" } });
  assert.equal(first.type, "model");
  assert.deepEqual(first.request.messages.at(-1), { role: "user", content: [{ type: "text", text: "list and read" }] });

  const calls = step(assistant([
    { type: "text", text: "on it" },
    { type: "tool_use", id: "c1", name: "ls", input: {} },
    { type: "tool_use", id: "c2", name: "read", input: { path: "a" } },
  ], "tool_use"));
  assert.equal(calls.type, "tools");
  assert.deepEqual(calls.calls.map((call) => call.callId), ["c1", "c2"]);

  // Completion order reversed; the transcript must keep assistant source order.
  const next = step({ type: "tools_completed", results: [
    { call_id: "c2", output: "contents", is_error: false },
    { call_id: "c1", output: "a b c", is_error: false },
  ] });
  assert.equal(next.type, "model");
  const results = next.request.messages.at(-1);
  assert.equal(results.role, "user");
  assert.deepEqual(results.content.map((block) => [block.tool_use_id, block.content]), [["c1", "a b c"], ["c2", "contents"]]);

  const reply = step(assistant([{ type: "text", text: "done" }]));
  assert.equal(reply.type, "emit");
  assert.equal(reply.event.content, "done");
});

test("fails a truncated tool batch without executing it", () => {
  const step = drive();
  step({ type: "user_message", input: { message: "go" } });
  const decision = step(assistant([
    { type: "tool_use", id: "c1", name: "write", input: { partial: true } },
  ], "max_tokens"));
  // Straight back to the model, never to the tools.
  assert.equal(decision.type, "model");
  const failure = decision.request.messages.at(-1);
  assert.equal(failure.content[0].type, "tool_result");
  assert.equal(failure.content[0].is_error, true);
  assert.match(failure.content[0].content, /Re-issue the tool call/u);
});

test("compacts older history into a structured checkpoint and keeps the recent tail", () => {
  // Tiny budgets so two messages cross the threshold.
  const step = drive({ contextWindow: 300, reserveTokens: 100, keepRecentTokens: 40 });
  step({ type: "user_message", input: { message: "old task ".repeat(60) } });
  step(assistant([{ type: "text", text: "old answer ".repeat(60) }]));

  const compaction = step({ type: "user_message", input: { message: "new question" } });
  assert.equal(compaction.type, "model");
  const prompt = compaction.request.messages.at(-1).content[0].text;
  assert.match(prompt, /## Goal/u);
  assert.match(prompt, /context checkpoint summary/u);

  const resumed = step(assistant([{ type: "text", text: "## Goal\nfinish the task" }]));
  assert.equal(resumed.type, "model");
  const rebuilt = resumed.request.messages;
  assert.match(rebuilt[0].content[0].text, /^Context checkpoint from earlier in this conversation:/u);
  assert.match(rebuilt[0].content[0].text, /finish the task/u);
  assert.deepEqual(rebuilt.at(-1), { role: "user", content: [{ type: "text", text: "new question" }] });
});
