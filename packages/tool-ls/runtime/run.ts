import { readdir } from "node:fs/promises";
import type { z } from "zod";
import type { inputSchema } from "../src/schema.js";
import { workspacePath, type RuntimeContext } from "../../../shared/tool-runtime.js";

export async function run(input: z.output<typeof inputSchema>, context: RuntimeContext) {
  const values = (await readdir(workspacePath(context.workspace, input.path), { withFileTypes: true }))
    .map((entry) => ({ name: entry.name, kind: entry.isDirectory() ? "dir" as const : "file" as const }));
  values.sort((left, right) => left.name.localeCompare(right.name));
  return { entries: values.slice(0, input.limit), truncated: values.length > input.limit };
}
