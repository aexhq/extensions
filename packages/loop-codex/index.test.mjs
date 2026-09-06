import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { brainEnv, inspectAgentloop, inspectComponent } from "@aexhq/brain";
import { codex } from "./dist/index.mjs";

test("publishes a raw Component factory with explicit placement", async () => {
  const bytes = await readFile(new URL("./dist/loop.component.wasm", import.meta.url));
  assert.deepEqual([...bytes.subarray(0, 4)], [0, 97, 115, 109]);
  const env = brainEnv({ name: "brain" });
  const binding = codex({ env });
  const source = inspectAgentloop(binding);
  assert.equal(source.environment, env);
  assert.deepEqual(source.configuration, { environmentSelection: "hidden", placements: {}, contextWindow: 200_000, compaction: true });
  assert.match(inspectComponent(source.implementation).artifact.pathname, /loop\.component\.wasm$/u);
  assert.throws(() => codex(), /requires \{ env \}/u);
});
