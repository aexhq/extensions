import { tool } from "@aexhq/brain";

import { inputSchema, outputSchema } from "./schema.js";

export const grep = tool({
  name: "grep",
  description: "Search text files in the Environment workspace with ripgrep.",
  input: inputSchema,
  output: outputSchema,
  implementation: Object.freeze({ type: "aex_official_tool", version: 1, name: "grep" }),
});
