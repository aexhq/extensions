import { tool } from "@aexhq/brain";

import { officialTool } from "./implementation.js";
import { grepInput, grepOutput } from "./schemas.js";

export const grep = tool({
  name: "grep",
  description: "Search text files in the Environment workspace with ripgrep.",
  input: grepInput,
  output: grepOutput,
  needs: ["process"],
  implementation: officialTool("grep"),
});
