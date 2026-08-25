import assert from "node:assert/strict";
import test from "node:test";
import { prepareComponent } from "@aexhq/brain";
import { app } from "./index.mjs";

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
