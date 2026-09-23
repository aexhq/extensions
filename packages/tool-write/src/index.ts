import { tool } from "@aexhq/brain";
import { run } from "../runtime/run.js";

import { inputSchema, outputSchema } from "./schema.js";

export const write = tool({
  name: "write",
  description: "Write UTF-8 text to a file in the Environment workspace, creating parent directories.",
  input: inputSchema,
  output: outputSchema,
  run: async (input, context) => context.finish(await run(input, { workspace: process.cwd(), signal: context.signal })),
});
