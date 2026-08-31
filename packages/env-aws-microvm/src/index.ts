import { spawn } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

import { CapabilityError, clamp, environment } from "@aexhq/brain";
import type { ExecOptions, ExecResult, FsHandle, ProvisionedToolArtifact } from "@aexhq/brain";
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

const DEFAULT_OUTPUT_BYTES_MAX = 1024 * 1024;
const KILL_GRACE_MS = 2_000;

/** The VM-side exec path: `bash -lc` in the guest, bounded by the attachment's
 * exec grant (a clamped timeout enforced by kill, capped captured output). */
function execute(command: string, opts: ExecOptions, outputBytesMax: number): Promise<ExecResult> {
  return new Promise((resolve, reject) => {
    const child = spawn("bash", ["-lc", command], {
      ...(opts.cwd === undefined ? {} : { cwd: opts.cwd }),
      env: opts.env === undefined ? process.env : { ...process.env, ...opts.env },
      stdio: [opts.stdin === undefined ? "ignore" : "pipe", "pipe", "pipe"],
    });
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
    if (opts.timeoutMs !== undefined) {
      const term = setTimeout(() => {
        child.kill("SIGTERM");
        const kill = setTimeout(() => child.kill("SIGKILL"), KILL_GRACE_MS);
        kill.unref();
        timers.push(kill);
      }, opts.timeoutMs);
      term.unref();
      timers.push(term);
    }
    let settled = false;
    const settle = (value: () => ExecResult | undefined): void => {
      if (settled) return;
      settled = true;
      for (const timer of timers) clearTimeout(timer);
      // Release the pipes so an orphaned grandchild cannot hold this process open.
      child.stdout.destroy();
      child.stderr.destroy();
      child.unref();
      const result = value();
      if (result !== undefined) resolve(result);
    };
    child.once("error", (error) => settle(() => { reject(error); return undefined; }));
    const finish = (code: number | null, signal: NodeJS.Signals | null) => (): ExecResult => ({
      exitCode: code ?? (signal === null ? -1 : 128 + (signal === "SIGKILL" ? 9 : 15)),
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
    if (opts.stdin !== undefined) child.stdin?.end(opts.stdin);
  });
}

/** The workspace-rooted fs provider. Every path is confined to the granted root
 * with `clamp.path`; writes create parent directories — dirty work stays here,
 * never in a Tool. */
function workspaceFs(root: string | undefined): FsHandle {
  const confined = (path: string): string => {
    if (root === undefined) throw new CapabilityError("fs", "not_granted", "this attachment carries no fs grant");
    return clamp.path(root, path);
  };
  return {
    read: (path) => readFile(confined(path)),
    async write(path, data) {
      const target = confined(path);
      await mkdir(dirname(target), { recursive: true });
      await writeFile(target, data);
    },
    async list(path) {
      const entries = await readdir(confined(path), { withFileTypes: true });
      return entries.map((entry) => ({ name: entry.name, kind: entry.isDirectory() ? ("dir" as const) : ("file" as const) }));
    },
  };
}

/** Provisioned artifacts this process can serve by content identity. The
 * deployed image points AEX_TOOL_ARTIFACT_DIR at its installed `*.tool.json`
 * artifacts (`brain build` output); without it the host serves none and an
 * attach naming an unknown identity fails its receipt. */
function installedArtifacts(): ProvisionedToolArtifact[] {
  const directory = process.env.AEX_TOOL_ARTIFACT_DIR;
  if (directory === undefined) return [];
  return readdirSync(directory)
    .filter((name) => name.endsWith(".tool.json"))
    .map((name) => JSON.parse(readFileSync(join(directory, name), "utf8")) as ProvisionedToolArtifact);
}

export const awsMicroVm = environment({ options }, (author) => {
  const vm = author.open(async () => ({}));
  vm.run(async () => { throw new Error("AWS MicroVM Tools execute in the Rust provider runtime"); });
  vm.close(async () => undefined);
  vm.provide.exec(({ grants }) => ({
    run: (command, opts) => execute(command, { ...(grants.fs?.root === undefined ? {} : { cwd: grants.fs.root }), ...clamp(opts, grants.exec) }, grants.exec?.output_bytes_max ?? DEFAULT_OUTPUT_BYTES_MAX),
  }));
  vm.provide.fs(({ grants }) => workspaceFs(grants.fs?.root));
  vm.host.esm({ artifacts: installedArtifacts() });
  return {
    suspend: vm.method(async () => undefined),
  };
});
