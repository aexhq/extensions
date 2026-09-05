import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { brainWasm, inspectAgentloop, inspectComponent } from "@aexhq/brain";
import { codex } from "./dist/index.mjs";

test("publishes a raw Component factory with explicit placement", async () => {
  const bytes = await readFile(new URL("./dist/loop.component.wasm", import.meta.url));
  assert.deepEqual([...bytes.subarray(0, 4)], [0, 97, 115, 109]);
  const env = brainWasm();
  const binding = codex({ env });
  const source = inspectAgentloop(binding);
  assert.equal(source.environment, env);
  assert.deepEqual(source.configuration, { contextWindow: 200_000, compaction: true });
  assert.match(inspectComponent(source.component).artifact.pathname, /loop\.component\.wasm$/u);
  assert.throws(() => codex(), /requires \{ env \}/u);
});
