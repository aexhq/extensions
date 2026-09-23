import { tool } from "@aexhq/brain";
import { run } from "../runtime/run.js";

import { inputSchema, outputSchema } from "./schema.js";

export const ls = tool({
  name: "ls",
  description: "List entries in an Environment workspace directory.",
  input: inputSchema,
  output: outputSchema,
  run: async (input, context) => context.finish(await run(input, { workspace: process.cwd(), signal: context.signal })),
});
