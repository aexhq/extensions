import { component } from "@aexhq/brain";

export function pi(options = {}) {
  return component(
    "agentloop",
    new URL("./dist/loop.component.wasm", import.meta.url),
    normalizeOptions(options, "pi"),
    { metadata: { name: "pi", source: "@aexhq/loop-pi" } },
  );
}

function normalizeOptions(options, name) {
  if (options === null || typeof options !== "object" || Array.isArray(options)) {
    throw new TypeError(`${name} options must be an object`);
  }
  const config = {};
  if (options.instructions !== undefined) {
    if (typeof options.instructions !== "string" || options.instructions.length > 131_072) {
      throw new TypeError(`${name} instructions must be a string of at most 131072 characters`);
    }
    config.instructions = options.instructions;
  }
  if (options.temperature !== undefined) {
    if (typeof options.temperature !== "number" || options.temperature < 0 || options.temperature > 2) {
      throw new TypeError(`${name} temperature must be between 0 and 2`);
    }
    config.temperature = options.temperature;
  }
  if (options.reasoningEffort !== undefined) {
    if (!["low", "medium", "high"].includes(options.reasoningEffort)) {
      throw new TypeError(`${name} reasoningEffort must be low, medium, or high`);
    }
    config.reasoningEffort = options.reasoningEffort;
  }
  return config;
}
