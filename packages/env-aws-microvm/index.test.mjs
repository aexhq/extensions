import assert from "node:assert/strict";
import test from "node:test";
import { inspectEnvironment } from "@aexhq/brain";
import { awsMicroVm } from "./dist/index.js";
const options = { name: "sandbox", url: "https://sandbox.example", token: "secret" };
test("creates a named HTTP Environment with separate credentials", () => {
  const source = inspectEnvironment(awsMicroVm({ ...options, region: "eu-west-2" }));
  assert.deepEqual(source.configuration, { driver: "aws-microvm", region: "eu-west-2" });
  assert.deepEqual(source.driver, { driver: "http", url: options.url, credential: "secret" });
  assert.equal(source.name, "sandbox");
});
test("validates provider configuration and leaves lifecycle policy with the caller", () => {
  assert.doesNotThrow(() => awsMicroVm(options));
  assert.throws(() => awsMicroVm({ ...options, region: "" }), /too small/iu);
  for (const field of ["idleSeconds", "maximumSeconds", "typo"]) {
    assert.throws(() => awsMicroVm({ ...options, [field]: 30 }), /unrecognized key/iu);
  }
});
