import assert from "node:assert/strict";
import test from "node:test";

import { environment, inspectTool } from "@aexhq/brain";
import { bash, edit, glob, grep, ls, read, todo, write } from "../dist/index.js";

const declarations = {
  bash: { factory: bash },
  edit: { factory: edit },
  glob: { factory: glob },
  grep: { factory: grep },
  ls: { factory: ls },
  read: { factory: read },
  todo: { factory: todo },
  write: { factory: write },
};

test("official Tool factories bind explicit Environments and opaque implementations", () => {
  const env = environment({ url: () => "https://test.example" })({ name: "test" });
  for (const [name, { factory }] of Object.entries(declarations)) {
    assert.throws(() => factory(), /requires \{ env \}/u, name);
    const source = inspectTool(factory({ env }));
    assert.equal(source.definition.name, name);
    assert.equal(source.environment, env);
    assert.equal("needs" in source, false);
    assert.deepEqual(source.implementation, { type: "aex_official_tool", version: 1, name });
  }
});

test("Tool factories reject options they do not declare", () => {
  const env = environment({ url: () => "https://test.example" })({ name: "test" });
  assert.throws(() => read({ env, typo: true }), /does not accept options/u);
});
