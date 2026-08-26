import assert from "node:assert/strict";
import test from "node:test";

import { stageOrder, versionSetByLatestChange } from "./npm-release.mjs";

test("a released package must take a new version with its latest change", () => {
  assert.equal(versionSetByLatestChange([
    "diff --git a/packages/model/package.json b/packages/model/package.json",
    '-  "version": "0.1.0",',
    '+  "version": "0.1.1",',
  ].join("\n")), true);

  // The package changed without a version, so a rebuild would silently ship under the released
  // one. A Wasm component never rebuilds byte-for-byte, so only this catches the missing bump.
  assert.equal(versionSetByLatestChange([
    "diff --git a/packages/model/package.json b/packages/model/package.json",
    '-  "description": "Bounded authoring utilities",',
    '+  "description": "Authoring utilities",',
  ].join("\n")), false);

  assert.equal(versionSetByLatestChange(""), false);
});

test("staging waves come from the release, not from a list someone maintains", () => {
  assert.deepEqual(stageOrder([
    { workspace: "model", name: "@aexhq/model", needs: [] },
    { workspace: "agentloop", name: "@aexhq/agentloop", needs: [] },
    { workspace: "model-openai", name: "@aexhq/model-openai", needs: ["model"] },
    { workspace: "loop-pi", name: "@aexhq/loop-pi", needs: ["agentloop"] },
  ]), { base: ["model", "agentloop"], dependents: ["model-openai", "loop-pi"] });

  // A dependent published before the exact version it installs is visible cannot resolve, so a
  // deeper chain has to add a wave rather than fan out anyway.
  assert.throws(() => stageOrder([
    { workspace: "model", name: "@aexhq/model", needs: [] },
    { workspace: "model-openai", name: "@aexhq/model-openai", needs: ["model"] },
    { workspace: "loop-pi", name: "@aexhq/loop-pi", needs: ["model-openai"] },
  ]), /@aexhq\/loop-pi needs model-openai/u);

  assert.throws(() => stageOrder([{ workspace: "model", name: "@aexhq/model" }]), /predates/u);
});
