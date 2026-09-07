import assert from "node:assert/strict";
import test from "node:test";

import { readFile } from "node:fs/promises";
import path from "node:path";

import { releasePlan } from "./npm-release.mjs";

const root = path.join(import.meta.dirname, "..");

test("an existing exact release trusts its immutable registry integrity without rebuilding", () => {
  assert.deepEqual(
    releasePlan("@aexhq/tools", "2.0.0", "sha512-registry"),
    {
      filename: "aexhq-tools-2.0.0.tgz",
      integrity: "sha512-registry",
      shouldPack: false,
    },
  );
});

test("an unpublished exact version requires a release archive", () => {
  assert.equal(releasePlan("@aexhq/tools", "2.0.0", undefined).shouldPack, true);
});

test("every public-repository package enables npm provenance", async () => {
  for (const workspace of ["env-aws-microvm", "loop-codex", "loop-pi", "tools"]) {
    const document = JSON.parse(await readFile(
      path.join(root, "packages", workspace, "package.json"),
      "utf8",
    ));
    assert.equal(document.publishConfig.provenance, true, document.name);
  }
});

test("the public-repository publisher requests npm provenance with an OIDC grant", async () => {
  const publisher = await readFile(path.join(root, "tools", "publish.mjs"), "utf8");
  const workflow = await readFile(path.join(root, ".github", "workflows", "npm-publish.yml"), "utf8");
  assert.match(publisher, /--provenance/u);
  assert.match(workflow, /id-token: write/u);
});
