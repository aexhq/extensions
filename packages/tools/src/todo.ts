import { tool } from "@aexhq/brain";
import { z } from "zod";

const item = z.object({ text: z.string().min(1), done: z.boolean().default(false) });

const todoInput = z.discriminatedUnion("action", [
    z.object({ action: z.literal("get") }),
    z.object({ action: z.literal("set"), items: z.array(item).max(200) }),
  ]);
const todoOutput = z.object({ items: z.array(item) });

export const todo = tool({
  description: "Read or replace the session's to-do list.",
  input: todoInput,
  output: todoOutput,
  requires: [],
}, (author) => {
  // Pure tool: the list lives in the provisioned module for the life of its
  // hosting Environment instance and touches no capability.
  let items: { text: string; done: boolean }[] = [];
  author.run(async (input) => {
    if (input.action === "set") items = input.items;
    return { items };
  });
});
