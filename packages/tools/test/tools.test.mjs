import assert from "node:assert/strict";
import test from "node:test";

import { MAX_SEALED_CONFIG_BYTES, prepareComponent, prepareComponents } from "@aexhq/brain";
import { bash, edit, glob, grep, ls, read, subagents, task, todo, write } from "../index.mjs";

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
    assert.equal(Object.isFrozen(value.config), true);
  }
  const prepared = await prepareComponent(values[0]);
  assert.ok(prepared.bytes > 0);
  assert.equal(prepared.component_digest.length, 64);
  assert.equal(prepared.bundle.checksum, values[0].config.descriptor.bundle_digest);
});

// Brain seals every component config into one CONFIG journal record and refuses the session when
// that record exceeds MAX_SEALED_CONFIG_BYTES. Inlining the ~283 KB Node bundle in each config took
// the full official set to about 3 MB, which only a live create could reveal. The budget is a
// quarter of the ceiling because the same record also carries the prompt, model and environment
// seals of whatever session selects these tools.
test("the full official Tool set seals far below Brain's CONFIG journal ceiling", async () => {
  const values = [bash(), edit(), glob(), grep(), ls(), read(), todo(), write(), subagents()];
  const sealed = values.reduce((total, value) => total + JSON.stringify(value.config).length, 0);
  assert.ok(
    sealed < MAX_SEALED_CONFIG_BYTES / 4,
    `the official Tool configs seal ${sealed} bytes against a ${MAX_SEALED_CONFIG_BYTES}-byte ceiling`,
  );

  const { bindings, toolArtifactLayers } = await prepareComponents(values);
  for (const [index, binding] of bindings.entries()) {
    const expected = values[index].config.descriptor?.bundle_digest;
    assert.equal(binding.bundle_digest, expected);
  }
  assert.equal(toolArtifactLayers.length, 8);
  for (const layer of toolArtifactLayers) {
    assert.equal(layer.media_type, "application/javascript+esm");
    assert.ok(layer.bytes > 0);
  }
});

test("subagents is an ordinary Tool component with child-session authority", async () => {
  const value = subagents();
  assert.equal(value.config.definition.name, "subagents");
  assert.deepEqual(value.grants, ["children"]);
  assert.deepEqual(task(), value);
  const prepared = await prepareComponent(value);
  assert.ok(prepared.bytes > 0);
  assert.equal(prepared.component_digest.length, 64);
});

test("each factory reuses one executable component and seals distinct configuration", () => {
  assert.deepEqual(bash().asset, read().asset);
  assert.notEqual(bash().config.definition.contract_digest, read().config.definition.contract_digest);
  assert.deepEqual(bash(), bash());
});
