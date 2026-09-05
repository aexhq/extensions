import { tool } from "@aexhq/brain";

import { officialTool } from "./implementation.js";
import { globInput, globOutput } from "./schemas.js";

export const glob = tool({
  name: "glob",
  description: "List Environment workspace paths matching a glob pattern.",
  input: globInput,
  output: globOutput,
  needs: ["fs"],
  implementation: officialTool("glob"),
});
