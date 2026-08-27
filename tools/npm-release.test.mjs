import assert from "node:assert/strict";
import test from "node:test";

import { releasedSourceCommit } from "./npm-release.mjs";

test("reads the Extensions commit from SLSA provenance", () => {
  const commit = "0123456789abcdef0123456789abcdef01234567";
  const payload = Buffer.from(JSON.stringify({
    predicate: {
      buildDefinition: {
        resolvedDependencies: [{
          uri: `git+https://github.com/aexhq/extensions@refs/tags/release/sha-${commit}`,
          digest: { gitCommit: commit },
        }],
      },
    },
  })).toString("base64");
  assert.equal(releasedSourceCommit({
    attestations: [{
      predicateType: "https://slsa.dev/provenance/v1",
      bundle: { dsseEnvelope: { payload } },
    }],
  }), commit);
});

test("rejects provenance from another repository", () => {
  const payload = Buffer.from(JSON.stringify({
    predicate: {
      buildDefinition: {
        resolvedDependencies: [{
          uri: "git+https://github.com/example/extensions@refs/heads/main",
          digest: { gitCommit: "0123456789abcdef0123456789abcdef01234567" },
        }],
      },
    },
  })).toString("base64");
  assert.throws(() => releasedSourceCommit({
    attestations: [{
      predicateType: "https://slsa.dev/provenance/v1",
      bundle: { dsseEnvelope: { payload } },
    }],
  }), /no Extensions source commit/u);
});
