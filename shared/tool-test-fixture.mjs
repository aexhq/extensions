import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { environment, inspectTool } from "@aexhq/brain";

export async function workspace(t) {
  const directory = await mkdtemp(join(tmpdir(), "tool-runtime-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return { workspace: directory, signal: AbortSignal.timeout(10_000) };
}

export function toolTests(name, factory, runtime, invalidInput) {
  test(`${name}: the factory binds an explicit Environment and opaque implementation`, () => {
    const env = environment({ url: () => "https://test.example" })({ name: "test" });
    assert.throws(() => factory(), /requires \{ env \}/u);
    const source = inspectTool(factory({ env }));
    assert.equal(source.definition.name, name);
    assert.equal(source.environment, env);
    assert.equal("needs" in source, false);
    assert.deepEqual(source.implementation, { type: "aex_official_tool", version: 1, name });
    assert.throws(() => factory({ env, typo: true }), /does not accept options/u);
  });

  test(`${name}: the built runtime rejects invalid input before execution`, async t => {
    await assert.rejects(runtime.execute(invalidInput, await workspace(t)), error => error.name === "ZodError");
  });
}
