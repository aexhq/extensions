import { tool } from "@aexhq/brain";

import { officialTool } from "./implementation.js";
import { editInput, editOutput } from "./schemas.js";

export const edit = tool({
  name: "edit",
  description: "Replace one exact occurrence of text in an Environment workspace file.",
  input: editInput,
  output: editOutput,
  needs: ["fs"],
  implementation: officialTool("edit"),
});
