import assert from "node:assert/strict";
import test from "node:test";
import { inspectEnvironment } from "@aexhq/environment";
import { awsMicrovm } from "./index.mjs";

test("awsMicrovm declares the computer profile without a language runtime", () => {
  assert.deepEqual(inspectEnvironment(awsMicrovm()).serialized, {
    extension: "@aexhq/env-aws-microvm",
    protocol: "environment/v1",
    profile: { kind: "computer", platform: "linux-amd64", network: "allowlist", recovery: "retained" },
    configuration: {},
  });
});
