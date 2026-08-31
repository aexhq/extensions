import { tool } from "@aexhq/brain";
import { z } from "zod";

const writeInput = z.object({ path: z.string().min(1), content: z.string() });
const writeOutput = z.object({ path: z.string(), bytes: z.number().int().nonnegative() });

export const write = tool({
  description: "Write UTF-8 text to a file in the Environment workspace, creating parent directories.",
  input: writeInput,
  output: writeOutput,
  requires: ["fs"],
}, (author) => {
  author.run(async ({ path, content }, context) => {
    await context.fs.write(path, content);
    return { path, bytes: new TextEncoder().encode(content).byteLength };
  });
});
