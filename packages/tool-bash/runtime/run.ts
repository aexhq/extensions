import { execFile } from "node:child_process";
import { promisify } from "node:util";
import type { z } from "zod";
import type { inputSchema } from "../src/schema.js";
import type { RuntimeContext } from "../../../shared/tool-runtime.js";

export async function run(input: z.output<typeof inputSchema>, context: RuntimeContext) {
  try {
    const { stdout, stderr } = await promisify(execFile)("bash", ["-lc", input.command], {
      signal: context.signal,
      cwd: context.workspace,
      maxBuffer: 16 * 1024 * 1024,
    });
    return { exit_code: 0, stdout, stderr };
  } catch (error) {
    const failure = error as { code?: unknown; stdout?: string; stderr?: string; message?: string };
    if (typeof failure.code === "number") {
      return { exit_code: failure.code, stdout: failure.stdout ?? "", stderr: failure.stderr ?? "" };
    }
    throw new Error(failure.stderr?.trim() || failure.message || "bash failed");
  }
}
