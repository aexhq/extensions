import assert from "node:assert/strict";
import test from "node:test";
import { awsMicrovm } from "./index.mjs";

test("awsMicrovm declares a stable remote Environment requirement", () => {
  const value = awsMicrovm({ id: "workspace-main", region: "eu-west-2", idleSeconds: 30, maximumSeconds: 600 });
  assert.deepEqual(value, {
    environment_id: "workspace-main",
    configuration: { driver: "aws-microvm", region: "eu-west-2", idle_seconds: 30, maximum_seconds: 600 },
    lifecycle_policy: "session",
  });
});

test("awsMicrovm rejects invalid configuration", () => {
  assert.throws(() => awsMicrovm({ id: "env", region: "" }), /non-empty/u);
  assert.throws(() => awsMicrovm([]), /options must be an object/u);
  assert.throws(() => awsMicrovm({ id: "env", idleSeconds: 0 }), /positive safe integer/u);
  assert.throws(
    () => awsMicrovm({ id: "env", idleSeconds: 61, maximumSeconds: 60 }),
    /cannot exceed/u,
  );
  assert.throws(() => awsMicrovm({ id: "env", typo: true }), /unknown/u);
  assert.throws(() => awsMicrovm({}), /stable Environment identifier/u);
});
