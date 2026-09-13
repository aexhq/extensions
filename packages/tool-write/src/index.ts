import { tool } from "@aexhq/brain";

import { inputSchema, outputSchema } from "./schema.js";

export const write = tool({
  name: "write",
  description: "Write UTF-8 text to a file in the Environment workspace, creating parent directories.",
  input: inputSchema,
  output: outputSchema,
  implementation: Object.freeze({ type: "aex_official_tool", version: 1, name: "write" }),
});
