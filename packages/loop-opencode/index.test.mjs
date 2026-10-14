import assert from "node:assert/strict";
import test from "node:test";
import { opencode } from "./src/index.mjs";
test("client selection serializes without executable code", () => {
  const options = { contextWindow: 128000 };
  assert.deepEqual(JSON.parse(JSON.stringify(opencode(options))), { name: "opencode", configuration: options });
  assert.deepEqual(opencode(), { name: "opencode", configuration: {} });
});
