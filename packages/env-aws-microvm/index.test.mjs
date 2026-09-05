import assert from "node:assert/strict";
import test from "node:test";

import { inspectEnvironment } from "@aexhq/brain";
import { awsMicroVm } from "./dist/index.js";

test("creates an immutable AWS MicroVM placement descriptor", () => {
  const source = inspectEnvironment(awsMicroVm({ region: "eu-west-2", idleSeconds: 30, maximumSeconds: 600 }));
  assert.deepEqual(source.configuration, {
    driver: "aws-microvm",
    region: "eu-west-2",
    idle_seconds: 30,
    maximum_seconds: 600,
  });
  assert.deepEqual(source.bindings, {});
});

test("validates provider options through the declared schema", () => {
  assert.doesNotThrow(() => awsMicroVm());
  assert.throws(() => awsMicroVm({ region: "" }), /too small/iu);
  assert.throws(() => awsMicroVm({ idleSeconds: 0 }), /too small/iu);
  assert.throws(() => awsMicroVm({ idleSeconds: 61, maximumSeconds: 60 }), /cannot exceed/u);
  assert.throws(() => awsMicroVm({ typo: true }), /unrecognized key/iu);
});
