import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { brainEnv, inspectEnvironment, inspectAgentloop, inspectComponent } from "@aexhq/brain";
import { pi } from "./dist/index.mjs";

test("publishes a raw Component factory with explicit placement", async () => {
  const bytes = await readFile(new URL("./dist/loop.component.wasm", import.meta.url));
  assert.deepEqual([...bytes.subarray(0, 4)], [0, 97, 115, 109]);
  const env = brainEnv({ name: "brain" });
  const binding = pi({ env });
  const source = inspectAgentloop(binding);
  assert.equal(source.environment, env);
  assert.deepEqual(source.configuration, { environmentSelection: "hidden", placements: {}, contextWindow: 200_000, reserveTokens: 16_384, keepRecentTokens: 20_000, compaction: true });
  assert.match(inspectComponent(source.implementation).artifact.pathname, /loop\.component\.wasm$/u);
  for (const binding of [pi(), pi({}), pi({ contextWindow: 64_000 })]) {
    const selected = inspectAgentloop(binding);
    assert.equal(inspectEnvironment(selected.environment).driver.driver, "brain");
    assert.equal(inspectEnvironment(selected.environment).name, "brain");
    assert.equal(selected.implementation, source.implementation);
  }
  assert.equal(inspectAgentloop(pi({ contextWindow: 64_000 })).configuration.contextWindow, 64_000);
  assert.throws(() => pi({ contextWindow: -1 }));
  assert.throws(() => pi({ unknown: true }));
  assert.throws(() => pi({ env: null }));
  for (const options of [null, [], 1]) assert.throws(() => pi(options), TypeError);
});
