import { tool } from "@aexhq/brain";
import { run } from "../runtime/run.js";

import { inputSchema, outputSchema } from "./schema.js";

export const grep = tool({
  name: "grep",
  description: "Search text files in the Environment workspace with ripgrep.",
  input: inputSchema,
  output: outputSchema,
  run: async (input, context) => context.finish(await run(input, { workspace: process.cwd(), signal: context.signal })),
});
