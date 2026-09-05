import { tool } from "@aexhq/brain";

import { officialTool } from "./implementation.js";
import { readInput, readOutput } from "./schemas.js";

export const read = tool({
  name: "read",
  description: "Read UTF-8 text from a file in the Environment workspace.",
  input: readInput,
  output: readOutput,
  needs: ["fs"],
  implementation: officialTool("read"),
});
