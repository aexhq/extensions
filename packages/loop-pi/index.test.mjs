import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";

import { pi } from "./index.mjs";

test("pi seals factory policy into deterministic bundle bytes", () => {
  const first = pi({ instructions: "Verify the change.", reasoningEffort: "high" });
  const second = pi({ instructions: "Verify the change.", reasoningEffort: "high" });
  assert.deepEqual(first, second);
  assert.equal(createHash("sha256").update(first.source).digest("hex"), first.sha256);
  assert.match(first.source, /Verify the change/);
  assert.doesNotMatch(first.source, /__AEX_LOOP_CONFIG_JSON__/);
});

test("pi rejects unsupported policy before session creation", () => {
  assert.throws(() => pi({ reasoningEffort: "extreme" }), /low, medium, or high/);
  assert.throws(() => pi({ temperature: 3 }), /between 0 and 2/);
});
