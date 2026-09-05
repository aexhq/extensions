import { tool } from "@aexhq/brain";

import { officialTool } from "./implementation.js";
import { bashInput, bashOutput } from "./schemas.js";

export const bash = tool({
  name: "bash",
  description: "Run a Bash command in the session Environment workspace.",
  input: bashInput,
  output: bashOutput,
  needs: ["process"],
  implementation: officialTool("bash"),
});
