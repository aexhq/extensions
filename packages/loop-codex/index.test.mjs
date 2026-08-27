import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { packageUrl } from "./index.mjs";

test("publishes a universal Agentloop package", async () => {
  const value = JSON.parse(await readFile(packageUrl, "utf8"));
  assert.equal(value.manifest.contract_version, "agentloop/v1");
  assert.match(value.manifest.component_digest, /^[0-9a-f]{64}$/u);
});
