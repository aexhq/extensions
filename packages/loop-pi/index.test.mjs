import assert from "node:assert/strict";
import test from "node:test";
import { piHarness } from "@aexhq/brain/harness/pi";
import { pi } from "./src/index.mjs";

test("exports the upstream pi harness without a second loop implementation", () => {
  assert.equal(pi, piHarness);
  const harness = pi();
  assert.equal(harness.name, "pi");
  assert.equal(typeof harness.open, "function");
});
