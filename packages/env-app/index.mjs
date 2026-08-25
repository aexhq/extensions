import { component } from "@aexhq/brain";

export function app(options) {
  if (
    options === null ||
    typeof options !== "object" ||
    !/^[A-Za-z0-9_.:-]{1,128}$/u.test(options.id)
  ) {
    throw new TypeError(
      "app({ id }) requires 1 through 128 letters, digits, dots, colons, underscores, or hyphens",
    );
  }
  return component(
    "environment",
    new URL("./dist/environment.component.wasm", import.meta.url),
    { driver: "customer", configuration: { registration: options.id } },
    { metadata: { name: "app", source: "@aexhq/env-app" } },
  );
}

export function callback(definition, registration = `tool:${definition?.contract_digest ?? ""}`) {
  if (
    definition === null ||
    typeof definition !== "object" ||
    !/^[A-Za-z_][A-Za-z0-9_-]{0,63}$/u.test(definition.name) ||
    !/^[0-9a-f]{64}$/u.test(definition.contract_digest) ||
    definition.input_schema === null ||
    typeof definition.input_schema !== "object" ||
    Array.isArray(definition.input_schema)
  ) {
    throw new TypeError("callback(definition) requires a valid Tool definition");
  }
  if (!/^[A-Za-z0-9_.:-]{1,128}$/u.test(registration)) {
    throw new TypeError("callback registration is invalid");
  }
  return component(
    "tool",
    new URL("./dist/tool.component.wasm", import.meta.url),
    {
      definition,
      descriptor: {
        registration,
        name: definition.name,
        contract_digest: definition.contract_digest,
      },
    },
    {
      grants: ["environment"],
      metadata: { name: definition.name, source: "@aexhq/env-app" },
    },
  );
}
