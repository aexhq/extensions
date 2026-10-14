import assert from "node:assert/strict";
import test from "node:test";
import { codexHarness } from "@aexhq/brain/harness/codex";
import { codex } from "./src/index.mjs";

test("exports the upstream codex harness without a second loop implementation", () => {
  assert.equal(codex, codexHarness);
  const harness = codex();
  assert.equal(harness.name, "codex");
  assert.equal(typeof harness.open, "function");
});
