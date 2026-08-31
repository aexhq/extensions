import { tool } from "@aexhq/brain";
import { z } from "zod";

const grepInput = z.object({ pattern: z.string().min(1), path: z.string().default("."), limit: z.number().int().positive().max(10_000).default(1_000) });
const grepOutput = z.object({ matches: z.array(z.string()), truncated: z.boolean() });

const quoted = (value: string): string => `'${value.replaceAll("'", String.raw`'\''`)}'`;

// ripgrep gives this tool its gitignore awareness, binary detection, and speed;
// none of that is expressible over the v1 fs handle, so grep stays on exec.
export const grep = tool({
  description: "Search text files in the Environment workspace with ripgrep.",
  input: grepInput,
  output: grepOutput,
  requires: ["exec"],
}, (author) => {
  author.run(async ({ pattern, path, limit }, context) => {
    const result = await context.exec.run(`rg --line-number --no-heading --color never --regexp ${quoted(pattern)} -- ${quoted(path)}`);
    if (result.exitCode !== 0 && result.exitCode !== 1) {
      throw new Error(result.stderr.trim() || `ripgrep exited ${result.exitCode}`);
    }
    const lines = result.stdout.split(/\r?\n/u).filter(Boolean);
    return { matches: lines.slice(0, limit), truncated: lines.length > limit };
  });
});
