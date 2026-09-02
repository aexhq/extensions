import { spawn } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

import { environment } from "@aexhq/brain";
import type { ProvisionedToolArtifact } from "@aexhq/brain";
import { z } from "zod";

const options = z.object({
  region: z.string().min(1).optional(),
  idleSeconds: z.number().int().positive().optional(),
  maximumSeconds: z.number().int().positive().optional(),
}).strict().default({}).refine((value) => value.idleSeconds === undefined || value.maximumSeconds === undefined || value.idleSeconds <= value.maximumSeconds, {
  message: "idleSeconds cannot exceed maximumSeconds",
}).transform((value) => ({
  ...(value.region === undefined ? {} : { region: value.region }),
  ...(value.idleSeconds === undefined ? {} : { idle_seconds: value.idleSeconds }),
  ...(value.maximumSeconds === undefined ? {} : { maximum_seconds: value.maximumSeconds }),
}));

/** The workspace every program starts in. The guest mounts it at `/workspace`; a
 * local test points the variable at a temporary directory. */
const WORKSPACE_ROOT = process.env.AEX_WORKSPACE_ROOT ?? "/workspace";
const OUTPUT_BYTES_MAX = 1024 * 1024;
const KILL_GRACE_MS = 2_000;

interface ShellResult { readonly exit_code: number; readonly stdout: string; readonly stderr: string }

/** Run a shell program in the guest: `bash -lc` in the workspace, killed at the
 * call's deadline or on cancellation, captured output capped. */
function execute(script: string, call: { readonly cwd: string; readonly timeoutMs: number; readonly signal: AbortSignal }, outputBytesMax: number): Promise<ShellResult> {
  return new Promise((resolve, reject) => {
    const child = spawn("bash", ["-lc", script], { cwd: call.cwd, env: process.env, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = Buffer.alloc(0);
    let stderr = Buffer.alloc(0);
    const capture = (target: "stdout" | "stderr") => (chunk: Buffer): void => {
      const current = target === "stdout" ? stdout : stderr;
      const next = Buffer.concat([current, chunk.subarray(0, Math.max(0, outputBytesMax - current.byteLength))]);
      if (target === "stdout") stdout = next;
      else stderr = next;
    };
    child.stdout.on("data", capture("stdout"));
    child.stderr.on("data", capture("stderr"));
    const timers: ReturnType<typeof setTimeout>[] = [];
    const kill = (): void => {
      child.kill("SIGTERM");
      const force = setTimeout(() => child.kill("SIGKILL"), KILL_GRACE_MS);
      force.unref();
      timers.push(force);
    };
    const term = setTimeout(kill, call.timeoutMs);
    term.unref();
    timers.push(term);
    call.signal.addEventListener("abort", kill, { once: true });
    let settled = false;
    const settle = (value: () => ShellResult | undefined): void => {
      if (settled) return;
      settled = true;
      for (const timer of timers) clearTimeout(timer);
      call.signal.removeEventListener("abort", kill);
      // Release the pipes so an orphaned grandchild cannot hold this process open.
      child.stdout.destroy();
      child.stderr.destroy();
      child.unref();
      const result = value();
      if (result !== undefined) resolve(result);
    };
    child.once("error", (error) => settle(() => { reject(error); return undefined; }));
    const finish = (code: number | null, signal: NodeJS.Signals | null) => (): ShellResult => ({
      exit_code: code ?? (signal === null ? -1 : 128 + (signal === "SIGKILL" ? 9 : 15)),
      stdout: stdout.toString("utf8"),
      stderr: stderr.toString("utf8"),
    });
    child.once("close", (code, signal) => settle(finish(code, signal)));
    // A killed shell can leave a grandchild holding the pipes, so `close` may
    // never fire; settle from `exit` after a short flush grace instead.
    child.once("exit", (code, signal) => {
      const flush = setTimeout(() => settle(finish(code, signal)), 250);
      flush.unref();
      timers.push(flush);
    });
  });
}

/** Built artifacts this process can serve by content identity. The deployed
 * image points AEX_TOOL_ARTIFACT_DIR at its installed `*.tool.json` artifacts
 * (`brain build` output); without it the host serves none and an attach naming
 * an unknown esm identity fails its receipt. */
function installedArtifacts(): ProvisionedToolArtifact[] {
  const directory = process.env.AEX_TOOL_ARTIFACT_DIR;
  if (directory === undefined) return [];
  return readdirSync(directory)
    .filter((name) => name.endsWith(".tool.json"))
    .map((name) => JSON.parse(readFileSync(join(directory, name), "utf8")) as ProvisionedToolArtifact);
}

export const awsMicroVm = environment(
  {
    options,
    // What a program finds on the VM. Enforcement is the guest's: the workspace
    // mount, the tool user, and the egress gateway — never a wrapper here.
    resources: {
      fs: { root: WORKSPACE_ROOT },
      process: { output_bytes_max: OUTPUT_BYTES_MAX },
    },
  },
  (author) => {
    const vm = author.open(async () => {
      // Every program starts in the workspace. Esm programs run in this process,
      // so the process itself moves there when the workspace is mounted.
      try { process.chdir(WORKSPACE_ROOT); } catch { /* not mounted here: relative paths stay where the host runs */ }
      return {};
    });
    vm.execute.esm({ artifacts: installedArtifacts() });
    vm.execute.shell(({ deadline, signal }, script) => execute(script, { cwd: WORKSPACE_ROOT, timeoutMs: Math.max(1, deadline.getTime() - Date.now()), signal }, OUTPUT_BYTES_MAX));
    vm.close(async () => undefined);
    return {
      suspend: vm.method(async () => undefined),
    };
  },
);
