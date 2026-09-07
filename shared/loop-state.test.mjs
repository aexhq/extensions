import assert from "node:assert/strict";
import test from "node:test";
import { runPi } from "../packages/loop-pi/src/logic.mjs";
import { runCodex } from "../packages/loop-codex/src/logic.mjs";

for (const run of [runPi, runCodex]) {
  const input = (configuration = {}) => ({ input: { message: "continue", media: [{ type: "image", url: "https://example.com/view.png" }] }, transcript: [], kv: {}, configuration, tools: [] });
  const host = () => {
    const saved = { transcript: [], kv: {} };
    return { saved, events: (after) => ({ events: [], next_cursor: after }),
      setTranscript: (messages) => { saved.transcript = structuredClone(messages); },
      setKv: (key, value) => { saved.kv[key] = structuredClone(value); },
      emit() {}, telemetry() {},
    };
  };
  test(`${run.name} saves input before model failure`, async () => {
    const context = host();
    context.model = () => { throw new Error("provider unavailable"); };
    await assert.rejects(run(input(), context), /provider unavailable/u);
    assert.equal(context.saved.transcript[0].content[1].type, "image");
    assert.equal(context.saved.kv.observed_sequence, 0);
  });
  test(`${run.name} preserves native state through saved continuation`, async () => {
    const context = host();
    const native = { type: "native", format: "anthropic.messages.v1", data: { type: "thinking", thinking: "reasoning", signature: "signed" } };
    context.model = () => ({ message: { role: "assistant", content: [native, { type: "text", text: "answer" }] }, stop_reason: "end_turn", usage: {} });
    await run(input({ compaction: false }), context);
    const restored = JSON.parse(JSON.stringify(context.saved));
    context.model = (request) => {
      assert.deepEqual(request.messages[1].content[0], native);
      return { message: { role: "assistant", content: [{ type: "text", text: "continued" }] }, stop_reason: "end_turn", usage: {} };
    };
    await run({ ...input({ compaction: false }), ...restored }, context);
  });
  for (const stop_reason of ["max_tokens", "refusal", "unknown"]) {
    test(`${run.name} keeps original context when compaction stops with ${stop_reason}`, async () => {
      const context = host();
      const original = { role: "user", content: [{ type: "text", text: "original task ".repeat(100) }] };
      context.model = (request) => {
        assert.equal(request.response_format, null);
        assert.deepEqual(request.tools, []);
        return { message: { role: "assistant", content: [{ type: "text", text: "partial summary" }] }, stop_reason, usage: {} };
      };
      await assert.rejects(run({ ...input({ contextWindow: 100, reserveTokens: 10, keepRecentTokens: 1 }), transcript: [original] }, context), /Compaction did not complete/u);
      assert.deepEqual(context.saved.transcript[0], original);
      assert.equal(context.saved.transcript.length, 2);
      assert.equal(context.saved.kv.checkpoint, undefined);
    });
  }
}
