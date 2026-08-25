import assert from "node:assert/strict";
import test from "node:test";

import { versionSetByLatestChange } from "./npm-release.mjs";

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
