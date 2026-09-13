import { mkdir, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import type { z } from "zod";
import type { inputSchema } from "../src/schema.js";
import { workspacePath, type RuntimeContext } from "../../../shared/tool-runtime.js";

export async function run(input: z.output<typeof inputSchema>, context: RuntimeContext) {
  const target = workspacePath(context.workspace, input.path);
  await mkdir(dirname(target), { recursive: true });
  await writeFile(target, input.content);
  return { path: input.path, bytes: new TextEncoder().encode(input.content).byteLength };
}
