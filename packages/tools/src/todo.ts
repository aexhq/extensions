import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

import { tool } from "@aexhq/brain";
import { z } from "zod";

import { workspaceOf, workspacePath } from "./path.js";

const item = z.object({ text: z.string().min(1), done: z.boolean().default(false) });

const todoInput = z.discriminatedUnion("action", [
    z.object({ action: z.literal("get") }),
    z.object({ action: z.literal("set"), items: z.array(item).max(200) }),
  ]);
const todoOutput = z.object({ items: z.array(item) });

export const todo = tool({ description: "Read or replace the session's portable to-do list.", input: todoInput, output: todoOutput }, (author) => {
  author.run(async (input, context) => {
    const path = workspacePath(workspaceOf(context), ".brain/todo.json");
    if (input.action === "set") {
      await mkdir(dirname(path), { recursive: true });
      await writeFile(path, `${JSON.stringify(input.items, null, 2)}\n`, "utf8");
      return { items: input.items };
    }
    try {
      return { items: z.array(item).parse(JSON.parse(await readFile(path, "utf8"))) };
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return { items: [] };
      throw error;
    }
  });
});
