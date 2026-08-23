import assert from "node:assert/strict";
import test from "node:test";
import { inspectEnvironment } from "@aexhq/environment";
import { app } from "./index.mjs";

test("app declares the callback profile and process id", () => {
  assert.deepEqual(inspectEnvironment(app({ id: "billing-api" })).serialized, {
    extension: "@aexhq/env-app",
    protocol: "environment/v1",
    profile: { kind: "callbacks", network: "unrestricted", recovery: "connection" },
    configuration: { id: "billing-api" },
  });
});
