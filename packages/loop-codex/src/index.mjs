import { agentloop, component, program } from "@aexhq/brain";
import { z } from "zod";

const options = z.object({
  environmentSelection: z.enum(["hidden", "model"]).default("hidden"),
  placements: z.record(z.string(), z.string()).default({}),
  contextWindow: z.number().int().positive().default(200_000),
  compaction: z.boolean().default(true),
  output: z.strictObject({ schema: z.union([z.boolean(), z.record(z.string(), z.unknown())]),
    maxCorrections: z.number().int().min(0).max(10).default(2) }).optional(),
}).strict();

export const codex = agentloop({
  options,
  implementation: program({
    runtime: component(new URL("./runtime.component.wasm", import.meta.url)),
    source: new URL("./loop.program.js", import.meta.url),
  }),
});
