import { tool } from "@aexhq/brain";

import { inputSchema, outputSchema } from "./schema.js";

export const read = tool({
  name: "read",
  description: "Read UTF-8 text from a file in the Environment workspace.",
  input: inputSchema,
  output: outputSchema,
  implementation: Object.freeze({ type: "aex_official_tool", version: 1, name: "read" }),
});
