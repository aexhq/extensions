const dialect = { $schema: "https://json-schema.org/draft/2020-12/schema", type: "object" };
export const schemaFixtures = [
  { name: "conditional", schema: { ...dialect,
    properties: { mode: { enum: ["single", "batch"] }, value: { type: "string" }, values: { type: "array", items: { type: "string" } } },
    required: ["mode"], additionalProperties: false,
    if: { properties: { mode: { const: "single" } } }, then: { required: ["value"] }, else: { required: ["values"] },
  }, valid: [{ mode: "single", value: "Ada" }, { mode: "batch", values: ["Lin"] }], invalid: [{ mode: "single" }, { mode: "batch", value: "Ada" }] },
  { name: "dependent", schema: { ...dialect,
    properties: { card: { type: "number" }, address: { type: "string" }, country: { type: "string", default: "GB" } },
    dependentRequired: { card: ["address"] }, dependentSchemas: { address: { required: ["country"] } },
  }, valid: [{}, { card: 123, address: "London", country: "GB", extra: "preserved" }], invalid: [{ card: 123 }, { address: "London" }, { card: "123", address: "London", country: "GB" }] },
  { name: "unevaluated", schema: { ...dialect,
    allOf: [{ properties: { value: { type: "string" } }, required: ["value"] }], unevaluatedProperties: false,
  }, valid: [{ value: "Ada" }], invalid: [{ value: "Ada", extra: true }] },
  { name: "recursive", schema: { ...dialect, $id: "https://example.test/tree.json",
    properties: { root: { $ref: "#/$defs/node" } }, required: ["root"], additionalProperties: false,
    $defs: { node: { type: "object", properties: { value: { type: "string" }, children: { type: "array", items: { $ref: "#/$defs/node" } } }, required: ["value"], additionalProperties: false } },
  }, valid: [{ root: { value: "Ada", children: [{ value: "Lin" }] } }], invalid: [{ root: { value: "Ada", children: [{ value: 3 }] } }] },
];
