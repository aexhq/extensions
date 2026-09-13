import { tool } from "@aexhq/brain";

import { inputSchema, outputSchema } from "./schema.js";

export const edit = tool({
  name: "edit",
  description: "Replace one exact occurrence of text in an Environment workspace file.",
  input: inputSchema,
  output: outputSchema,
  implementation: Object.freeze({ type: "aex_official_tool", version: 1, name: "edit" }),
});
