import assert from "node:assert/strict";
import test from "node:test";
import { codex } from "./src/index.mjs";
test("client selection serializes without executable code", () => {
  const options = { contextWindow: 128000 };
  assert.deepEqual(JSON.parse(JSON.stringify(codex(options))), { name: "codex", configuration: options });
  assert.deepEqual(codex(), { name: "codex", configuration: {} });
});
