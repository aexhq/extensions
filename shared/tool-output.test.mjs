import assert from "node:assert/strict";
import test from "node:test";
import { toolOutput, toolResult, resultBlock } from "./tool-output.mjs";

test("official media presentation preserves ordinary JSON and failure status", () => {
  const image = { type: "image", url: "https://example.com/image.png" };
  const output = toolOutput({ answer: 42 }, [image]);
  assert.deepEqual(resultBlock("call", { output, is_error: false }), {
    type: "tool_result", tool_use_id: "call", content: { answer: 42 }, media: [image], is_error: false,
  });
  assert.equal(resultBlock("call", { output, is_error: true }).content, output);
  assert.equal(resultBlock("call", { output, is_error: true }).is_error, true);
  const ordinary = { media: [image] };
  assert.equal(resultBlock("call", { output: ordinary, is_error: false }).content, ordinary);
  assert.throws(() => toolResult("call", undefined), /dispatch omitted/u);
  assert.throws(() => resultBlock("call", { output: toolOutput({}, [{ type: "audio" }]), is_error: false }), /image or PDF URL blocks/u);
});
