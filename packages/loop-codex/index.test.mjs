import assert from "node:assert/strict";
import test from "node:test";
import { prepareComponent } from "@aexhq/brain";

import { codex } from "./index.mjs";

test("codex returns one precompiled Agentloop declaration with sealed policy", async () => {
  const first = codex({ instructions: "Prefer small changes.", temperature: 0.2 });
  const second = codex({ instructions: "Prefer small changes.", temperature: 0.2 });
  assert.deepEqual(first, second);
  assert.equal(first.extension, "agentloop");
  assert.deepEqual(first.config, { instructions: "Prefer small changes.", temperature: 0.2 });
  const wire = await prepareComponent(first);
  assert.equal(wire.component_digest.length, 64);
  assert.ok(wire.bytes > 0);
});

test("codex rejects unsupported policy before session creation", () => {
  assert.throws(() => codex({ instructions: 1 }), /instructions must be a string/);
  assert.throws(() => codex(null), /options must be an object/);
});
