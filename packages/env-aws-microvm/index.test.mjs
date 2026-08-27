import assert from "node:assert/strict";
import test from "node:test";

import { awsMicroVm } from "./index.mjs";

test("defaults to a session-scoped workspace Environment", () => {
  const value = awsMicroVm({ region: "eu-west-2", idleSeconds: 30, maximumSeconds: 600 });
  assert.equal(value.kind, "environment");
  assert.equal(value.capability, "workspace");
  assert.deepEqual(value.configuration, {
    driver: "aws-microvm",
    region: "eu-west-2",
    idle_seconds: 30,
    maximum_seconds: 600,
  });
  assert.deepEqual(value.lifecycle, {});
});

test("requires identity only for a shared or external Environment", () => {
  assert.deepEqual(awsMicroVm({ lifecycle: "shared", id: "workspace-main" }).lifecycle, {
    type: "shared",
    id: "workspace-main",
  });
  assert.throws(() => awsMicroVm({ lifecycle: "shared" }), /requires a stable id/u);
  assert.throws(() => awsMicroVm({ id: "unnecessary" }), /cannot declare an id/u);
});

test("rejects invalid provider configuration", () => {
  assert.throws(() => awsMicroVm({ region: "" }), /non-empty/u);
  assert.throws(() => awsMicroVm([]), /options must be an object/u);
  assert.throws(() => awsMicroVm({ idleSeconds: 0 }), /positive safe integer/u);
  assert.throws(() => awsMicroVm({ idleSeconds: 61, maximumSeconds: 60 }), /cannot exceed/u);
  assert.throws(() => awsMicroVm({ typo: true }), /unknown/u);
});
