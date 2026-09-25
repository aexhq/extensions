import assert from "node:assert/strict";
import test from "node:test";
import { inspectTool } from "@aexhq/brain";
import { env } from "../dist/packages/env/src/index.js";

test("env is an ordinary Tool that uses only its supplied Environment service", async () => {
  const reference = { name: "workspace", sequence: 12 };
  const calls = [];
  const context = { environments: Object.fromEntries(["list", "get", "create", "update", "setup", "delete", "call"].map(operation => [operation, async (...args) => {
    calls.push({ operation, args }); return { operation };
  }])), finish: async value => value };
  const source = inspectTool(env({ environments: [{ environment: "workspace", permissions: ["read", "call"], methods: ["inspect"] }] }));
  assert.equal(source.definition.name, "env");
  for (const request of [
    { operation: "list" }, { operation: "get", environment: reference },
    { operation: "create", template: "workspace", name: "second", configuration: {} },
    { operation: "update", environment: reference, configuration: {} },
    { operation: "setup", environment: reference }, { operation: "delete", environment: reference },
    { operation: "call", environment: reference, method: "inspect", input: { resource: "job" } },
  ]) assert.deepEqual(await source.handler(request, context), { operation: request.operation });
  assert.deepEqual(calls.at(-1), { operation: "call", args: [reference, "inspect", { resource: "job" }] });
  assert.deepEqual(source.environments[0].permissions, ["read", "call"]);
});
