import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { brainEnv, inspectAgentloop, inspectComponent, inspectProgram } from "@aexhq/brain";
import { codex } from "./dist/index.mjs";

test("publishes a program and a reusable runtime with explicit placement", async () => {
  const bytes = await readFile(new URL("./dist/runtime.component.wasm", import.meta.url));
  assert.deepEqual([...bytes.subarray(0, 4)], [0, 97, 115, 109]);
  const env = brainEnv({ name: "brain" });
  const binding = codex({ env });
  const source = inspectAgentloop(binding);
  assert.equal(source.environment, env);
  assert.deepEqual(source.configuration, { environmentSelection: "hidden", placements: {}, contextWindow: 200_000, compaction: true });
  const program = inspectProgram(source.implementation);
  assert.match(inspectComponent(program.runtime).artifact.pathname, /runtime\.component\.wasm$/u);
  assert.match(program.artifact.pathname, /loop\.program\.js$/u);
  assert.ok((await readFile(program.artifact)).length < bytes.length / 2);
  assert.throws(() => codex(), /requires \{ env \}/u);
});
