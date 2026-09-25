import assert from "node:assert/strict";
import test from "node:test";
import Ajv from "ajv/dist/2020.js";
import { toolPlacement, refreshTools } from "./tool-placement.mjs";

const definition = { name: "read", description: "Read", input_schema: { type: "object" } };
const tools = [{ ...definition, environments: ["browser", "sandbox"] }];
const call = { name: "read", call_id: "one", input: { path: "file" } };

test("model-visible selection preserves recursive schema references and literal data", () => {
  const input_schema = { type: "object", properties: {
    next: { $ref: "#" }, value: { $ref: "#/$defs/value" },
    literal: { const: { $ref: "#/literal" } },
  }, $defs: { value: { type: "string" } }, additionalProperties: false };
  const placement = toolPlacement([{ ...definition, input_schema, environments: ["workspace"] }], { environmentSelection: "model" });
  const validate = new Ajv({ strict: true }).compile(placement.definitions[0].input_schema);
  assert.equal(validate({ environment: "workspace", input: { next: { value: "nested" }, literal: { $ref: "#/literal" } } }), true);
  assert.equal(validate({ environment: "workspace", input: { next: { value: 42 } } }), false);
  assert.equal(input_schema.properties.next.$ref, "#");
});

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

test("live views replace deleted placements and pin dynamic incarnations without leaking metadata to the model", async () => {
  const views = [
    { reference: { name: "sandbox", sequence: 1 }, template: "sandbox", state: "deleted", tools: [definition] },
    { reference: { name: "replacement", sequence: 42 }, template: "sandbox", state: "ready", tools: [definition] },
  ];
  const current = await refreshTools(tools, { environments: { list: async () => views } });
  assert.deepEqual(current[0].environments, ["browser", "replacement"]);
  const placement = toolPlacement(current, { placements: { read: "replacement" } });
  assert.deepEqual(placement.definitions, [definition]);
  assert.deepEqual(placement.invocation(call), { ...call, environment: "replacement", environment_sequence: 42 });
  views[1].state = "deleted";
  const emptied = await refreshTools([{ ...definition, environments: ["sandbox"] }], { environments: { list: async () => views } });
  assert.deepEqual(toolPlacement(emptied).definitions, []);
});
