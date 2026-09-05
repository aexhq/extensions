import { tool } from "@aexhq/brain";

import { officialTool } from "./implementation.js";
import { writeInput, writeOutput } from "./schemas.js";

export const write = tool({
  name: "write",
  description: "Write UTF-8 text to a file in the Environment workspace, creating parent directories.",
  input: writeInput,
  output: writeOutput,
  needs: ["fs"],
  implementation: officialTool("write"),
});
