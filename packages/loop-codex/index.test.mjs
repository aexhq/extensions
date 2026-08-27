import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { codex } from "./dist/index.mjs";

test("publishes a callback-authored Brain package", async () => {
  const value = JSON.parse(await readFile(new URL("./dist/codex.brain.json", import.meta.url), "utf8"));
  assert.equal(value.manifest.contract_version, "agentloop/v1");
  assert.match(value.manifest.component_digest, /^[0-9a-f]{64}$/u);
  assert.deepEqual(Object.keys(codex()), []);
});
