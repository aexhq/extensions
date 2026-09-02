import { readdir } from "node:fs/promises";

import { tool } from "@aexhq/brain";
import { z } from "zod";

const lsInput = z.object({ path: z.string().default("."), limit: z.number().int().positive().max(10_000).default(1_000) });
const lsOutput = z.object({ entries: z.array(z.object({ name: z.string(), kind: z.enum(["file", "dir"]) })), truncated: z.boolean() });

export const ls = tool({
  description: "List entries in an Environment workspace directory.",
  input: lsInput,
  output: lsOutput,
  needs: ["fs"],
}, (author) => {
  author.run(async ({ path, limit }) => {
    const values = (await readdir(path, { withFileTypes: true })).map((entry) => ({ name: entry.name, kind: entry.isDirectory() ? ("dir" as const) : ("file" as const) }));
    values.sort((left, right) => left.name.localeCompare(right.name));
    return { entries: values.slice(0, limit), truncated: values.length > limit };
  });
});
