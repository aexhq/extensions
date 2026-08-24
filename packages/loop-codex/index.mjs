import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";

const identity = JSON.parse(
  await readFile(new URL("./dist/identity.json", import.meta.url), "utf8"),
);

const baseSource = await readFile(new URL("./dist/loop.bundle.mjs", import.meta.url), "utf8");
const marker = '"__AEX_LOOP_CONFIG_JSON__"';

export function codex(options = {}) {
  const config = normalizeOptions(options, "codex");
  const source = baseSource.replace(marker, JSON.stringify(JSON.stringify(config)));
  if (source === baseSource) throw new Error("codex bundle is missing its configuration marker");
  return Object.freeze({
    source,
    sha256: createHash("sha256").update(source, "utf8").digest("hex"),
    toolchain: identity.toolchain,
  });
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
