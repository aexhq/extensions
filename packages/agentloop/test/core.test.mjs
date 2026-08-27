import assert from "node:assert/strict";
import test from "node:test";

import { defineAgentloop } from "../dist/index.js";
import { step } from "../dist/testing.js";

test("defines one synchronous pure step contract", () => {
  const loop = defineAgentloop({
    step(input) {
      input.context.items.push("changed");
      return { context: input.context, decision: { type: "finish" } };
    },
  });
  const input = {
    context: { protocolVersion: "agentloop/v1", items: [] },
    observation: { type: "user_message", content: "hello" },
    presentation: { bytes: new Uint8Array(), digest: "a".repeat(64) },
    runtime: { logicalTimeMs: 1n, deterministicSeed: new Uint8Array() },
  };
  assert.equal(step(loop, input).decision.type, "finish");
  assert.deepEqual(input.context.items, []);
  assert.ok(Object.isFrozen(loop));
});

test("rejects an object without a step function", () => {
  assert.throws(() => defineAgentloop({}), /step function/u);
});
