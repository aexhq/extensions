import { tool } from "@aexhq/brain";

import { officialTool } from "./implementation.js";
import { lsInput, lsOutput } from "./schemas.js";

export const ls = tool({
  name: "ls",
  description: "List entries in an Environment workspace directory.",
  input: lsInput,
  output: lsOutput,
  needs: ["fs"],
  implementation: officialTool("ls"),
});
