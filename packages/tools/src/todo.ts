import { tool } from "@aexhq/brain";

import { officialTool } from "./implementation.js";
import { todoInput, todoOutput } from "./schemas.js";

export const todo = tool({
  name: "todo",
  description: "Read or replace the session's to-do list.",
  input: todoInput,
  output: todoOutput,
  needs: ["fs"],
  implementation: officialTool("todo"),
});
