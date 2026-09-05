import { agentloop, component } from "@aexhq/brain";
import { z } from "zod";

const options = z.object({
  contextWindow: z.number().int().positive().default(200_000),
  compaction: z.boolean().default(true),
}).strict();

export const codex = agentloop({
  options,
  implementation: component(new URL("./loop.component.wasm", import.meta.url)),
});
