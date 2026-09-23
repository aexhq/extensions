import { tool } from "@aexhq/brain";
import { run } from "../runtime/run.js";

import { inputSchema, outputSchema } from "./schema.js";

export const read = tool({
  name: "read",
  description: "Read UTF-8 text from a file in the Environment workspace.",
  input: inputSchema,
  output: outputSchema,
  run: async (input, context) => context.finish(await run(input, { workspace: process.cwd(), signal: context.signal })),
});
