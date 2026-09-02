import { readFile } from "node:fs/promises";

import { tool } from "@aexhq/brain";
import { z } from "zod";

const readInput = z.object({
  path: z.string().min(1),
  offset: z.number().int().nonnegative().default(0),
  limit: z.number().int().positive().max(1024 * 1024).default(256 * 1024),
});
const readOutput = z.object({ content: z.string(), bytes: z.number().int().nonnegative(), truncated: z.boolean() });

export const read = tool({
  description: "Read UTF-8 text from a file in the Environment workspace.",
  input: readInput,
  output: readOutput,
  needs: ["fs"],
}, (author) => {
  author.run(async ({ path, offset, limit }) => {
    const file = await readFile(path);
    const data = file.subarray(offset, offset + limit);
    if (data.includes(0)) throw new Error(`${path} is binary`);
    return { content: new TextDecoder().decode(data), bytes: data.byteLength, truncated: file.byteLength > offset + limit };
  });
});
