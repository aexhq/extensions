import assert from "node:assert/strict";
import test from "node:test";
import { SseDecoder, typed, usage } from "./index.mjs";

test("SSE decoding survives every byte split", () => {
  const wire = new TextEncoder().encode("event: answer\r\ndata: {\"text\":\"é\"}\r\n\r\ndata: [DONE]\n\n");
  for (let split = 1; split < wire.byteLength; split += 1) {
    const decoder = new SseDecoder();
    const events = [...decoder.feed(wire.subarray(0, split)), ...decoder.feed(wire.subarray(split))];
    assert.deepEqual(events, [
      { event: "answer", data: "{\"text\":\"é\"}" },
      { event: undefined, data: "[DONE]" },
    ]);
    assert.equal(decoder.pending, 0);
  }
});

test("SSE frames are bounded and absent usage stays absent", () => {
  const decoder = new SseDecoder(8);
  assert.throws(() => decoder.feed(new Uint8Array(9)), /exceeded 8 bytes/u);
  assert.deepEqual(usage({ inputTokens: 4, outputTokens: undefined }), { inputTokens: 4 });
});

const caught = (call) => { try { call(); } catch (error) { return error; } throw new Error('the call did not fail'); };

test('a failed export reports the typed contract error instead of an opaque trap', () => {
  assert.deepEqual(caught(() => typed('observe', () => { throw new Error('OpenAI HTTP status 401'); })).payload, {
    code: 'model_observe_failed',
    message: 'OpenAI HTTP status 401',
    retryable: false,
  });
  assert.equal(typed('observe', () => 'ok'), 'ok');
});

test("a host import's own typed failure is preserved exactly", () => {
  const hostFailure = { payload: { code: 'network_denied', message: 'blocked', retryable: false } };
  assert.equal(caught(() => typed('start', () => { throw hostFailure; })), hostFailure);
});

test("SSE decoding accepts any byte sequence the component ABI delivers", () => {
  const wire = [...new TextEncoder().encode("data: {\"a\":1}\n\n")];
  // A host list<u8> does not reach a componentized guest as a Uint8Array.
  assert.deepEqual(new SseDecoder().feed(wire), [{ event: undefined, data: "{\"a\":1}" }]);
  assert.deepEqual(new SseDecoder().feed(Uint8Array.from(wire)), [{ event: undefined, data: "{\"a\":1}" }]);
  assert.throws(() => new SseDecoder().feed("data: x"), /byte sequence/u);
});
