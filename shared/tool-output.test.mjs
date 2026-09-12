import assert from "node:assert/strict";
import test from "node:test";
import { toolOutput, toolResult } from "./tool-output.mjs";

test("official media presentation preserves ordinary JSON and failure status", () => {
  const image = { type: "image", url: "https://example.com/image.png" };
  const output = toolOutput({ answer: 42 }, [image]);
  assert.deepEqual(toolResult("call", { output, is_error: false }), {
    type: "tool_result", tool_use_id: "call", content: { answer: 42 }, media: [image], is_error: false,
  });
  assert.equal(toolResult("call", { output, is_error: true }).content, output);
  assert.equal(toolResult("call", { output, is_error: true }).is_error, true);
  const ordinary = { media: [image] };
  assert.equal(toolResult("call", { output: ordinary, is_error: false }).content, ordinary);
  assert.equal(toolResult("call", undefined).is_error, true);
  assert.throws(() => toolResult("call", { output: toolOutput({}, [{ type: "audio" }]), is_error: false }), /image blocks/u);
});
