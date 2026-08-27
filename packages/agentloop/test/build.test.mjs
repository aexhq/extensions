import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { buildAgentloop } from "../dist/build.js";

test("builds an opaque portable Agentloop package without exposing WIT", async () => {
  const built = await buildAgentloop({
    entry: fileURLToPath(new URL("./fixtures/diagnostic-loop.mjs", import.meta.url)),
  });
  assert.equal(built.manifest.contract_version, "agentloop/v1");
  assert.match(built.manifest.component_digest, /^[0-9a-f]{64}$/u);
  assert.ok(built.manifest.component_bytes > 0);
  assert.ok(Buffer.from(built.component_base64, "base64").byteLength > 0);
});
