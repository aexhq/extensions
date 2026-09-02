import { mkdir, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

import { tool } from "@aexhq/brain";
import { z } from "zod";

const writeInput = z.object({ path: z.string().min(1), content: z.string() });
const writeOutput = z.object({ path: z.string(), bytes: z.number().int().nonnegative() });

export const write = tool({
  description: "Write UTF-8 text to a file in the Environment workspace, creating parent directories.",
  input: writeInput,
  output: writeOutput,
  needs: ["fs"],
}, (author) => {
  author.run(async ({ path, content }) => {
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, content);
    return { path, bytes: new TextEncoder().encode(content).byteLength };
  });
});
