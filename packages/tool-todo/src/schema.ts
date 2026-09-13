import { z } from "zod";

const todoItem = z.object({ text: z.string().min(1), done: z.boolean().default(false) });
export const inputSchema = z.discriminatedUnion("action", [
  z.object({ action: z.literal("get") }),
  z.object({ action: z.literal("set"), items: z.array(todoItem).max(200) }),
]);
export const outputSchema = z.object({ items: z.array(todoItem) });
