import { execFile } from "node:child_process";
import { promisify } from "node:util";

import { tool } from "@aexhq/brain";
import { z } from "zod";

const grepInput = z.object({ pattern: z.string().min(1), path: z.string().default("."), limit: z.number().int().positive().max(10_000).default(1_000) });
const grepOutput = z.object({ matches: z.array(z.string()), truncated: z.boolean() });

// ripgrep gives this tool its gitignore awareness, binary detection, and speed.
// It is a system binary of the environment's image, reached through a process.
export const grep = tool({
  description: "Search text files in the Environment workspace with ripgrep.",
  input: grepInput,
  output: grepOutput,
  needs: ["process"],
}, (author) => {
  author.run(async ({ pattern, path, limit }, context) => {
    let stdout: string;
    try {
      ({ stdout } = await promisify(execFile)("rg", ["--line-number", "--no-heading", "--color", "never", "--regexp", pattern, "--", path], { signal: context.signal, maxBuffer: 16 * 1024 * 1024 }));
    } catch (error) {
      const failure = error as { code?: unknown; stderr?: string; message?: string };
      // Exit 1 is ripgrep's "no matches": an ordinary empty result.
      if (failure.code === 1) return { matches: [], truncated: false };
      throw new Error(failure.stderr?.trim() || failure.message || "ripgrep failed");
    }
    const lines = stdout.split(/\r?\n/u).filter(Boolean);
    return { matches: lines.slice(0, limit), truncated: lines.length > limit };
  });
});
