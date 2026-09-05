import { agentloop, component } from "@aexhq/brain";
import { z } from "zod";
const options = z.object({
  contextWindow: z.number().int().positive().default(200_000),
  // pi defaults (compaction.ts): compact when context exceeds
  // contextWindow - reserveTokens, keep ~keepRecentTokens of recent messages.
  reserveTokens: z.number().int().positive().default(16_384),
  keepRecentTokens: z.number().int().positive().default(20_000),
  compaction: z.boolean().default(true),
}).strict();

export const pi = agentloop({
  options,
  implementation: component(new URL("./loop.component.wasm", import.meta.url)),
});
