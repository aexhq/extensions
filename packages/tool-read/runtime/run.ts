import { readFile } from "node:fs/promises";
import type { z } from "zod";
import type { inputSchema } from "../src/schema.js";
import { workspacePath, type RuntimeContext } from "../../../shared/tool-runtime.js";

export async function run(input: z.output<typeof inputSchema>, context: RuntimeContext) {
  const file = await readFile(workspacePath(context.workspace, input.path));
  const data = file.subarray(input.offset, input.offset + input.limit);
  if (data.includes(0)) throw new Error(`${input.path} is binary`);
  return {
    content: new TextDecoder().decode(data),
    bytes: data.byteLength,
    truncated: file.byteLength > input.offset + input.limit,
  };
}
