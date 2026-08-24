import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";

import { codex } from "./index.mjs";

test("codex seals factory policy into deterministic bundle bytes", () => {
  const first = codex({ instructions: "Prefer small changes.", temperature: 0.2 });
  const second = codex({ instructions: "Prefer small changes.", temperature: 0.2 });
  assert.deepEqual(first, second);
  assert.equal(createHash("sha256").update(first.source).digest("hex"), first.sha256);
  assert.match(first.source, /Prefer small changes/);
  assert.doesNotMatch(first.source, /__AEX_LOOP_CONFIG_JSON__/);
});

test("codex rejects unsupported policy before session creation", () => {
  assert.throws(() => codex({ instructions: 1 }), /instructions must be a string/);
  assert.throws(() => codex(null), /options must be an object/);
});
