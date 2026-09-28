import { agentloop, brainEnv, component } from "@aexhq/brain";
import { z } from "zod";
const options = z.object({
  environmentSelection: z.enum(["hidden", "model"]).default("hidden"),
  placements: z.record(z.string(), z.string()).default({}),
  contextWindow: z.number().int().positive().default(200_000),
  // pi defaults (compaction.ts): compact when context exceeds
  // contextWindow - reserveTokens, keep ~keepRecentTokens of recent messages.
  reserveTokens: z.number().int().positive().default(16_384),
  keepRecentTokens: z.number().int().positive().default(20_000),
  compaction: z.boolean().default(true),
  output: z.strictObject({ schema: z.union([z.boolean(), z.record(z.string(), z.unknown())]),
    maxCorrections: z.number().int().min(0).max(10).default(2) }).optional(),
}).strict();

const factory = agentloop({
  options,
  implementation: component(new URL("./loop.component.wasm", import.meta.url)),
});

export function pi(options = {}) {
  if (options === null || typeof options !== "object" || Array.isArray(options)) throw new TypeError("pi options must be an object");
  return factory({ ...options, env: options.env === undefined ? brainEnv({ name: "brain" }) : options.env });
}
