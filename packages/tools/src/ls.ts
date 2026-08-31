import { tool } from "@aexhq/brain";
import { z } from "zod";

const lsInput = z.object({ path: z.string().default("."), limit: z.number().int().positive().max(10_000).default(1_000) });
const lsOutput = z.object({ entries: z.array(z.object({ name: z.string(), kind: z.enum(["file", "dir"]) })), truncated: z.boolean() });

export const ls = tool({
  description: "List entries in an Environment workspace directory.",
  input: lsInput,
  output: lsOutput,
  requires: ["fs"],
}, (author) => {
  author.run(async ({ path, limit }, context) => {
    const values = [...await context.fs.list(path)];
    values.sort((left, right) => left.name.localeCompare(right.name));
    return {
      entries: values.slice(0, limit).map((value) => ({ name: value.name, kind: value.kind })),
      truncated: values.length > limit,
    };
  });
});
