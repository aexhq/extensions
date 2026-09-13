import { tool } from "@aexhq/brain";

import { inputSchema, outputSchema } from "./schema.js";

export const todo = tool({
  name: "todo",
  description: "Read or replace the session's to-do list.",
  input: inputSchema,
  output: outputSchema,
  implementation: Object.freeze({ type: "aex_official_tool", version: 1, name: "todo" }),
});
