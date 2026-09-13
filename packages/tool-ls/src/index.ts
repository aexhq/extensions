import { tool } from "@aexhq/brain";

import { inputSchema, outputSchema } from "./schema.js";

export const ls = tool({
  name: "ls",
  description: "List entries in an Environment workspace directory.",
  input: inputSchema,
  output: outputSchema,
  implementation: Object.freeze({ type: "aex_official_tool", version: 1, name: "ls" }),
});
