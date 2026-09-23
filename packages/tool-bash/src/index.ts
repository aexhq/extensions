import { tool } from "@aexhq/brain";
import { run } from "../runtime/run.js";

import { inputSchema, outputSchema } from "./schema.js";

export const bash = tool({
  name: "bash",
  description: "Run a Bash command in the session Environment workspace.",
  input: inputSchema,
  output: outputSchema,
  run: async (input, context) => context.finish(await run(input, { workspace: process.cwd(), signal: context.signal })),
});
