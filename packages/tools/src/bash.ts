import { tool } from "@aexhq/brain";
import { z } from "zod";

const bashInput = z.object({ command: z.string().min(1) });
const bashOutput = z.object({ exit_code: z.number().int(), stdout: z.string(), stderr: z.string() });

/** One shell program: the command is the script. The environment runs it in the
 * workspace and returns its exit code and captured output. */
export const bash = tool.shell({
  description: "Run a Bash command in the session Environment workspace.",
  input: bashInput,
  output: bashOutput,
  needs: ["process"],
  script: "$command",
});
