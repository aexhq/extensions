import { readFile, writeFile } from "node:fs/promises";
import type { z } from "zod";
import type { inputSchema } from "../src/schema.js";
import { workspacePath, type RuntimeContext } from "../../../shared/tool-runtime.js";

export async function run(input: z.output<typeof inputSchema>, context: RuntimeContext) {
  const target = workspacePath(context.workspace, input.path);
  const content = await readFile(target, "utf8");
  const first = content.indexOf(input.old_text);
  if (first < 0) throw new Error("old_text was not found");
  if (content.indexOf(input.old_text, first + input.old_text.length) >= 0) {
    throw new Error("old_text occurs more than once; provide a more specific match");
  }
  await writeFile(target, `${content.slice(0, first)}${input.new_text}${content.slice(first + input.old_text.length)}`);
  return { path: input.path, replacements: 1 };
}
