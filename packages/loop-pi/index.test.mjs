import assert from "node:assert/strict";
import test from "node:test";
import { prepareComponent } from "@aexhq/brain";

import { pi } from "./index.mjs";

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
