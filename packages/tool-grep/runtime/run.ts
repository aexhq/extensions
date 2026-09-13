import { execFile } from "node:child_process";
import { relative, resolve } from "node:path";
import { promisify } from "node:util";
import type { z } from "zod";
import type { inputSchema } from "../src/schema.js";
import { workspacePath, type RuntimeContext } from "../../../shared/tool-runtime.js";

export async function run(input: z.output<typeof inputSchema>, context: RuntimeContext) {
  let stdout: string;
  const target = relative(resolve(context.workspace), workspacePath(context.workspace, input.path)) || ".";
  try {
    ({ stdout } = await promisify(execFile)(
      "rg",
      ["--line-number", "--no-heading", "--color", "never", "--regexp", input.pattern, "--", target],
      { signal: context.signal, cwd: context.workspace, maxBuffer: 16 * 1024 * 1024 },
    ));
  } catch (error) {
    const failure = error as { code?: unknown; stderr?: string; message?: string };
    if (failure.code === 1) return { matches: [], truncated: false };
    throw new Error(failure.stderr?.trim() || failure.message || "ripgrep failed");
  }
  const lines = stdout.split(/\r?\n/u).filter(Boolean);
  return { matches: lines.slice(0, input.limit), truncated: lines.length > input.limit };
}
