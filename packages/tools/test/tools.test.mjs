import assert from "node:assert/strict";
import test from "node:test";

import { environment, inspectTool } from "@aexhq/brain";
import { bash, edit, glob, grep, ls, read, todo, write } from "../dist/index.js";

const declarations = {
  bash: { factory: bash, needs: ["pkg:apt/bash", "file:///workspace?access=write"] },
  edit: { factory: edit, needs: ["file:///workspace?access=write"] },
  glob: { factory: glob, needs: ["file:///workspace"] },
  grep: { factory: grep, needs: ["pkg:apt/ripgrep", "file:///workspace"] },
  ls: { factory: ls, needs: ["file:///workspace"] },
  read: { factory: read, needs: ["file:///workspace"] },
  todo: { factory: todo, needs: ["file:///workspace?access=write"] },
  write: { factory: write, needs: ["file:///workspace?access=write"] },
};

test("official Tool factories bind explicit Environments and opaque implementations", () => {
  const env = environment({ url: () => "https://test.example" })({ name: "test" });
  for (const [name, { factory, needs }] of Object.entries(declarations)) {
    assert.throws(() => factory(), /requires \{ env \}/u, name);
    const source = inspectTool(factory({ env }));
    assert.equal(source.definition.name, name);
    assert.equal(source.environment, env);
    assert.deepEqual(source.needs, needs);
    assert.deepEqual(source.implementation, { type: "aex_official_tool", version: 1, name });
  }
});

test("Tool factories reject options they do not declare", () => {
  const env = environment({ url: () => "https://test.example" })({ name: "test" });
  assert.throws(() => read({ env, typo: true }), /does not accept options/u);
});
