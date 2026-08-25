import assert from "node:assert/strict";
import test from "node:test";
import { AssistantMessageEventStream } from "@earendil-works/pi-ai";
import { prepareComponent } from "@aexhq/brain";

import { pi } from "./index.mjs";
import { erroredAssistantMessage } from "./src/loop.mjs";

test("pi returns one precompiled Agentloop declaration with sealed policy", async () => {
  const first = pi({ instructions: "Verify the change.", reasoningEffort: "high" });
  const second = pi({ instructions: "Verify the change.", reasoningEffort: "high" });
  assert.deepEqual(first, second);
  assert.equal(first.extension, "agentloop");
  assert.deepEqual(first.config, { instructions: "Verify the change.", reasoningEffort: "high" });
  const wire = await prepareComponent(first);
  assert.equal(wire.component_digest.length, 64);
  assert.ok(wire.bytes > 0);
});

test("pi rejects unsupported policy before session creation", () => {
  assert.throws(() => pi({ reasoningEffort: "extreme" }), /low, medium, or high/);
  assert.throws(() => pi({ temperature: 3 }), /between 0 and 2/);
});

test("a failed model round stops pi on the errored assistant message", async () => {
  const stream = new AssistantMessageEventStream();
  const message = erroredAssistantMessage("openai/gpt-4.1-nano", new Error("provider transport failed"));
  stream.push({ type: "error", reason: message.stopReason, error: message });
  stream.end();

  // pi reads blocks off this result and stops the loop on its reason, so a bare string here
  // crashes the activation instead of failing the turn with the provider's message.
  const result = await stream.result();
  assert.equal(result.stopReason, "error");
  assert.equal(result.errorMessage, "provider transport failed");
  assert.deepEqual(result.content, []);
});
