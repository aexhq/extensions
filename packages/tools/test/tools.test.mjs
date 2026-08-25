import assert from "node:assert/strict";
import test from "node:test";

import { prepareComponent } from "@aexhq/brain";
import { bash, edit, glob, grep, ls, read, todo, write } from "../index.mjs";

test("official tools are immutable Environment-routed components", async () => {
  const values = [bash(), edit(), glob(), grep(), ls(), read(), todo(), write()];
  assert.deepEqual(
    values.map((value) => value.config.definition.name),
    ["bash", "edit", "glob", "grep", "ls", "read", "todo", "write"],
  );
  for (const value of values) {
    assert.equal(value.extension, "tool");
    assert.deepEqual(value.grants, ["environment"]);
    assert.match(value.config.definition.contract_digest, /^[0-9a-f]{64}$/u);
    assert.match(value.config.descriptor.bundle_digest, /^[0-9a-f]{64}$/u);
    assert.equal(value.config.descriptor.runtime, "node22");
    assert.ok(value.config.bundleBase64.length > 0);
    assert.equal(Object.isFrozen(value.config), true);
  }
  const prepared = await prepareComponent(values[0]);
  assert.ok(prepared.bytes > 0);
  assert.equal(prepared.component_digest.length, 64);
});

test("each factory reuses one executable component and seals distinct configuration", () => {
  assert.deepEqual(bash().asset, read().asset);
  assert.notEqual(bash().config.definition.contract_digest, read().config.definition.contract_digest);
  assert.deepEqual(bash(), bash());
});
