import assert from "node:assert/strict";
import test from "node:test";
import { prepareComponent } from "@aexhq/brain";
import { app, callback } from "./index.mjs";

test("app declares a customer driver through the generic Environment world", async () => {
  const value = app({ id: "billing-api" });
  assert.equal(value.extension, "environment");
  assert.deepEqual(value.config, {
    driver: "customer",
    configuration: { registration: "billing-api" },
  });
  assert.ok((await prepareComponent(value)).bytes > 0);
});

test("app rejects an invalid registration", () => {
  assert.throws(() => app({ id: "not a registration!" }), /app\(\{ id \}\)/u);
});

test("callback declares a source-free Tool component over the Environment grant", () => {
  const definition = {
    name: "lookup",
    description: "Look up a record",
    input_schema: { type: "object" },
    output_schema: { type: "object" },
    contract_digest: "a".repeat(64),
  };
  const value = callback(definition);
  assert.equal(value.extension, "tool");
  assert.deepEqual(value.grants, ["environment"]);
  assert.deepEqual(value.config, {
    definition,
    descriptor: {
      registration: `tool:${"a".repeat(64)}`,
      name: "lookup",
      contract_digest: "a".repeat(64),
    },
  });
  assert.equal("handler" in value.config, false);
});
