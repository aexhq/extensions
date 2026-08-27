import assert from "node:assert/strict";
import test from "node:test";

import { awsMicroVm } from "./dist/index.mjs";

test("creates an opaque Environment with its own methods", async () => {
  const vm = awsMicroVm({ region: "eu-west-2", idleSeconds: 30, maximumSeconds: 600 });
  assert.deepEqual(Object.keys(vm), ["suspend"]);
  await assert.rejects(vm.suspend(), /only while its session is attached/u);
});

test("validates provider options through the declared schema", () => {
  assert.doesNotThrow(() => awsMicroVm());
  assert.throws(() => awsMicroVm({ region: "" }), /Too small/u);
  assert.throws(() => awsMicroVm({ idleSeconds: 0 }), /Too small/u);
  assert.throws(() => awsMicroVm({ idleSeconds: 61, maximumSeconds: 60 }), /cannot exceed/u);
  assert.throws(() => awsMicroVm({ typo: true }), /Unrecognized key/u);
});
