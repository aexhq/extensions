import assert from "node:assert/strict";
import test from "node:test";
import { pi } from "./src/index.mjs";
test("client selection serializes without executable code", () => {
  const options = { contextWindow: 128000 };
  assert.deepEqual(JSON.parse(JSON.stringify(pi(options))), { name: "pi", configuration: options });
  assert.deepEqual(pi(), { name: "pi", configuration: {} });
});
