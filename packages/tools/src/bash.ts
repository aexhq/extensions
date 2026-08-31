import { tool } from "@aexhq/brain";
import { z } from "zod";

const bashInput = z.object({
    command: z.string().min(1),
    cwd: z.string().optional(),
    timeout_ms: z.number().int().positive().optional(),
  });
const bashOutput = z.object({
    exit_code: z.number().int(),
    stdout: z.string(),
    stderr: z.string(),
  });

export const bash = tool({
  description: "Run a Bash command in the session Environment workspace.",
  input: bashInput,
  output: bashOutput,
  requires: ["exec"],
}, (author) => {
  author.run(async (input, context) => {
    const result = await context.exec.run(input.command, {
      ...(input.cwd === undefined ? {} : { cwd: input.cwd }),
      ...(input.timeout_ms === undefined ? {} : { timeoutMs: input.timeout_ms }),
    });
    return { exit_code: result.exitCode, stdout: result.stdout, stderr: result.stderr };
  });
});
