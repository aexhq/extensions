import assert from "node:assert/strict";
import test from "node:test";
import { runPi } from "../packages/loop-pi/src/logic.mjs";
import { runCodex } from "../packages/loop-codex/src/logic.mjs";
import { toolOutput, toolResult } from "./tool-output.mjs";

for (const run of [runPi, runCodex]) {
  test(`${run.name} acknowledges async results and finish without duplicating a provider reply`, async () => {
    const saved = { transcript: [], kv: {} };
    const history = [];
    const requests = [];
    const context = {
      environments: { list: async () => [] },
      kv: { read: key => saved.kv[key], put: (key, value) => { saved.kv[key] = value; } },
      events: after => ({ events: history.filter(event => event.sequence > after), next_cursor: history.at(-1)?.sequence ?? after }),
      acknowledge: through => { saved.kv["brain.last_activation"] = through; },
      setTranscript: messages => { saved.transcript = structuredClone(messages); },
      emit() {},
      dispatch: calls => {
        assert.equal(calls.length, 1);
        const returned = { sequence: 2, event_type: "tool_call_returned", data: { sequence: 1 } };
        history.push(returned);
        return [{ call_id: "call", sequence: 1, events: [returned], finished: false }];
      },
      model: request => {
        requests.push(structuredClone(request));
        return requests.length === 1
          ? { message: { role: "assistant", content: [{ type: "tool_use", id: "call", name: "work", input: {} }] }, stop_reason: "tool_use", usage: {} }
          : { message: { role: "assistant", content: [{ type: "text", text: "done for now" }] }, stop_reason: "end_turn", usage: {} };
      },
    };
    const base = { configuration: { compaction: false }, tools: [{ name: "work", environments: ["sandbox"], input_schema: {} }] };
    await run({ ...base, input: { message: "start" }, transcript: [], kv: {} }, context);
    assert.deepEqual(requests[1].messages.at(-1).content[0].content, { status: "running", results: [] });
    assert.equal(saved.kv["brain.last_activation"], 2);
    const image = { type: "image", url: "https://example.com/result.png" };
    history.push(
      { sequence: 3, event_type: "tool_result_emitted", data: { sequence: 1, result: { call_id: "call", is_error: false, output: toolOutput("later", [image]) } } },
      { sequence: 4, event_type: "tool_call_ended", data: { sequence: 1, outcome: { status: "ok", value: null } } },
    );
    await run({ ...base, ...structuredClone(saved) }, context);
    assert.equal(requests.length, 3);
    assert.equal(saved.kv["brain.last_activation"], 4);
    assert.equal(saved.transcript.flatMap(message => message.content).filter(block => block.type === "tool_result").length, 1);
    assert.deepEqual(requests[2].messages.flatMap(message => message.content).filter(block => block.type === "image"), [image]);
    assert.match(JSON.stringify(requests[2].messages), /later/u);

    history.push({ sequence: 5, event_type: "progress", data: {} });
    const before = structuredClone(saved.transcript);
    assert.deepEqual(await run({ ...base, ...structuredClone(saved) }, context), {});
    assert.equal(requests.length, 3, "a custom progress observation can be dismissed without a model call");
    assert.equal(saved.kv["brain.last_activation"], 5);
    assert.deepEqual(saved.transcript, before);
  });
}

test("dispatch presentation distinguishes no result, several results and terminal failure", () => {
  const result = (sequence, value) => ({ sequence, event_type: "tool_result_emitted", data: { result: { call_id: "call", output: value, is_error: false } } });
  const returned = { call_id: "call", sequence: 1, events: [], finished: false };
  assert.deepEqual(toolResult("call", returned).content, { status: "running", results: [] });
  assert.deepEqual(toolResult("call", { ...returned, finished: true }).content, { status: "finished", results: [] });
  const multiple = toolResult("call", { ...returned, events: [result(2, "one"), result(3, "two")], finished: true });
  assert.deepEqual(multiple.content.results.map(result => result.content), ["one", "two"]);
  const failed = toolResult("call", { ...returned, finished: true, events: [
    { sequence: 4, event_type: "tool_call_ended", data: { outcome: { status: "error", error: { code: "denied", message: "denied", details: { path: "private" } } } } },
  ] });
  assert.equal(failed.is_error, true);
  assert.deepEqual(failed.content, { code: "denied", message: "denied", details: { path: "private" } });
});

