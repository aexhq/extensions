import { tool } from "@aexhq/brain";

import { inputSchema, outputSchema } from "./schema.js";

export const bash = tool({
  name: "bash",
  description: "Run a Bash command in the session Environment workspace.",
  input: inputSchema,
  output: outputSchema,
  implementation: Object.freeze({ type: "aex_official_tool", version: 1, name: "bash" }),
});
