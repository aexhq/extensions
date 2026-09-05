import assert from "node:assert/strict";
import test from "node:test";

import { environment, inspectPlacedTool } from "@aexhq/brain";
import { bash, edit, glob, grep, ls, read, todo, write } from "../dist/index.js";

const declarations = {
  bash: { factory: bash, needs: ["process"] },
  edit: { factory: edit, needs: ["fs"] },
  glob: { factory: glob, needs: ["fs"] },
  grep: { factory: grep, needs: ["process"] },
  ls: { factory: ls, needs: ["fs"] },
  read: { factory: read, needs: ["fs"] },
  todo: { factory: todo, needs: ["fs"] },
  write: { factory: write, needs: ["fs"] },
};

test("official Tool factories bind explicit Environments and opaque implementations", () => {
  const env = environment({ driver: "test" })();
  for (const [name, { factory, needs }] of Object.entries(declarations)) {
    assert.throws(() => factory(), /requires \{ env \}/u, name);
    const source = inspectPlacedTool(factory({ env }));
    assert.equal(source.definition.name, name);
    assert.equal(source.environment, env);
    assert.deepEqual(source.needs, needs);
    assert.deepEqual(source.implementation, { type: "aex_official_tool", version: 1, name });
  }
});

test("Tool factories reject options they do not declare", () => {
  const env = environment({ driver: "test" })();
  assert.throws(() => read({ env, typo: true }), /does not accept options/u);
});
