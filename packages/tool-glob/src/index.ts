import { tool } from "@aexhq/brain";

import { inputSchema, outputSchema } from "./schema.js";

export const glob = tool({
  name: "glob",
  description: "List Environment workspace paths matching a glob pattern.",
  input: inputSchema,
  output: outputSchema,
  implementation: Object.freeze({ type: "aex_official_tool", version: 1, name: "glob" }),
});
