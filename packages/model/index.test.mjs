import assert from "node:assert/strict";
import test from "node:test";
import { SseDecoder, usage } from "./index.mjs";

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
