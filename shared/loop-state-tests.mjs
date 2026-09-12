import assert from "node:assert/strict";
import test from "node:test";

export function loopStateTests(run) {
  const input = (configuration = {}) => ({ input: { message: "continue", media: [{ type: "image", url: "https://example.com/view.png" }] }, transcript: [], kv: {}, configuration, tools: [] });
  const host = () => {
    const saved = { transcript: [], kv: {} };
    return { saved, events: (after) => ({ events: [], next_cursor: after }),
      setTranscript: (messages) => { saved.transcript = structuredClone(messages); },
      kv: {
        read: (key) => structuredClone(saved.kv[key]),
        put: (key, value) => { saved.kv[key] = structuredClone(value); },
        delete: (key) => { delete saved.kv[key]; },
      },
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
  for (const partial of [false, true]) {
    test(`${run.name} explains interrupted calls and preserves ${partial ? "partial" : "unanswered"} history`, async () => {
      const context = host();
      const native = { type: "native", format: "anthropic.messages.v1", data: { type: "thinking", thinking: "reasoning", signature: "signed" } };
      const assistant = { role: "assistant", content: [native, ...["image", "text"].map(id => ({ type: "tool_use", id, name: "lookup", input: {} }))] };
      const existing = { type: "tool_result", tool_use_id: "image", content: "saved image", media: [{ type: "image", url: "https://example.com/saved.png" }], is_error: false };
      const transcript = [assistant, ...(partial ? [{ role: "user", content: [existing, { type: "text", text: "saved note" }] }] : [])];
      const original = structuredClone(transcript);
      context.events = after => after === 0 ? { events: [
        { event_type: "tool_call_ended", data: { result: { call_id: "text", output: "not reconstructed from Events" } } },
        { event_type: "turn_failed", data: { code: "cancelled", message: "caller interrupted" } },
      ], next_cursor: 4 } : { events: [], next_cursor: after };
      context.dispatch = () => assert.fail("interrupted calls must not be redispatched");
      context.model = request => {
        assert.deepEqual(request.messages[0], assistant);
        const results = request.messages[1].content;
        assert.deepEqual(results.slice(0, 2).map(block => block.tool_use_id), ["image", "text"]);
        if (partial) {
          assert.deepEqual(results[0], existing);
          assert.equal(results[2].text, "saved note");
        }
        for (const result of results.slice(partial ? 1 : 0, 2)) {
          assert.equal(result.is_error, true);
          assert.match(result.content, /operation may have run/u);
          assert.match(result.content, /caller interrupted/u);
        }
        assert.equal(JSON.stringify(request.messages).includes("not reconstructed"), false);
        assert.equal(context.saved.kv.observed_sequence, 4);
        return { message: { role: "assistant", content: [{ type: "text", text: "continued" }] }, stop_reason: "end_turn", usage: {} };
      };
      await run({ ...input({ compaction: false }), transcript }, context);
      assert.deepEqual(transcript, original);
    });
  }
  test(`${run.name} does not advance observations when saving the transcript fails`, async () => {
    const context = host();
    context.events = after => after === 0
      ? { events: [{ event_type: "turn_failed", data: { message: "interrupted" } }], next_cursor: 7 }
      : { events: [], next_cursor: after };
    const save = context.setTranscript;
    context.setTranscript = () => { throw new Error("store unavailable"); };
    context.model = () => assert.fail("model must wait for saved observations");
    await assert.rejects(run(input(), context), /store unavailable/u);
    assert.equal(context.saved.kv.observed_sequence, undefined);
    context.setTranscript = save;
    context.model = () => { throw new Error("provider unavailable"); };
    await assert.rejects(run(input(), context), /provider unavailable/u);
    assert.match(context.saved.transcript[0].content[0].text, /interrupted/u);
    assert.equal(context.saved.kv.observed_sequence, 7);
  });
  test(`${run.name} saves Tool errors intact before a later model failure`, async () => {
    const context = host();
    const failure = { code: "unavailable", message: "choose another action", retryable: false, details: { environment: "sandbox" } };
    let modelCalls = 0;
    let dispatches = 0;
    context.model = () => {
      if (modelCalls++ === 0) return { message: { role: "assistant", content: [{ type: "tool_use", id: "call-1", name: "lookup", input: {} }] }, stop_reason: "tool_use", usage: {} };
      throw new Error("provider disconnected after Tool completion");
    };
    context.dispatch = () => { dispatches++; return [{ call_id: "call-1", output: failure, is_error: true }]; };
    await assert.rejects(run({ ...input({ compaction: false }), tools: [{ name: "lookup", environments: ["sandbox"], input_schema: { type: "object" } }] }, context), /provider disconnected/u);
    assert.equal(dispatches, 1);
    assert.deepEqual(context.saved.transcript.at(-1).content, [{ type: "tool_result", tool_use_id: "call-1", content: failure, is_error: true }]);
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
