import { mkdir, open, readFile, rename, unlink } from "node:fs/promises";
import type { z } from "zod";
import type { inputSchema } from "../src/schema.js";
import { workspacePath, type RuntimeContext } from "../../../shared/tool-runtime.js";

export async function run(input: z.output<typeof inputSchema>, context: RuntimeContext) {
  const directory = workspacePath(context.workspace, ".aex");
  const target = workspacePath(context.workspace, ".aex/todo.json");
  if (input.action === "set") {
    await mkdir(directory, { recursive: true });
    const temporary = workspacePath(context.workspace, ".aex/todo.pending");
    const file = await open(temporary, "wx").catch((error: NodeJS.ErrnoException) => {
      if (error.code === "EEXIST") {
        throw new Error("Todo write conflict: .aex/todo.pending exists. Another write is active or a previous write was interrupted; inspect the todo state before retrying.", { cause: error });
      }
      throw error;
    });
    try {
      try {
        await file.writeFile(JSON.stringify(input.items));
      } finally {
        await file.close();
      }
      await rename(temporary, target);
    } catch (error) {
      try {
        await unlink(temporary);
      } catch (cleanupError) {
        throw new AggregateError([error, cleanupError], `Todo write failed: ${error}; cleanup of .aex/todo.pending failed: ${cleanupError}`);
      }
      throw error;
    }
    return { items: input.items };
  }
  try {
    return { items: JSON.parse(await readFile(target, "utf8")) as { text: string; done: boolean }[] };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return { items: [] };
    throw error;
  }
}
