import assert from "node:assert/strict";
import test from "node:test";
import { toolPlacement } from "./tool-placement.mjs";

const definition = { name: "read", description: "Read", input_schema: { type: "object" } };
const tools = [{ ...definition, environments: ["browser", "sandbox"] }];
const call = { name: "read", call_id: "one", input: { path: "file" } };

test("hidden placement requires an explicit policy when several placements are authorized", () => {
  assert.throws(() => toolPlacement(tools).invocation(call), /Environment selection/u);
  const placement = toolPlacement(tools, { placements: { read: "sandbox" } });
  assert.deepEqual(placement.definitions, [definition]);
  assert.deepEqual(placement.invocation(call), { ...call, environment: "sandbox" });
  assert.throws(() => toolPlacement(tools, { placements: { read: "elsewhere" } }).invocation(call), /authorized/u);
});

test("model-visible presentation unwraps the selected Environment before canonical dispatch", () => {
  const placement = toolPlacement(tools, { environmentSelection: "model" });
  assert.deepEqual(placement.definitions[0].input_schema.properties.environment.enum, ["browser", "sandbox"]);
  assert.deepEqual(placement.invocation({ ...call, input: { environment: "browser", input: call.input } }), { ...call, environment: "browser" });
  assert.throws(() => placement.invocation({ ...call, input: { environment: "other", input: {} } }), /authorized/u);
});

test("valid Tool names do not inherit a placement from Object.prototype", () => {
  const placement = toolPlacement([{ ...definition, name: "constructor", environments: ["sandbox"] }]);
  assert.equal(placement.invocation({ ...call, name: "constructor" }).environment, "sandbox");
});
