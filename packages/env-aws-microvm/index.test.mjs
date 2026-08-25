import assert from "node:assert/strict";
import test from "node:test";
import { prepareComponent } from "@aexhq/brain";
import { awsMicrovm } from "./index.mjs";

test("awsMicrovm declares only its external driver configuration", async () => {
  const value = awsMicrovm({ region: "eu-west-2" });
  assert.equal(value.extension, "environment");
  assert.deepEqual(value.config, {
    driver: "aws-microvm",
    configuration: { region: "eu-west-2" },
  });
  assert.ok((await prepareComponent(value)).bytes > 0);
});

test("awsMicrovm rejects invalid configuration", () => {
  assert.throws(() => awsMicrovm({ region: "" }), /non-empty/u);
  assert.throws(() => awsMicrovm([]), /options must be an object/u);
});
