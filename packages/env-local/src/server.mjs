import { execFile, spawn } from "node:child_process";
import { promisify } from "node:util";
import { randomUUID } from "node:crypto";
import { mkdir, readFile, writeFile, rename } from "node:fs/promises";
import { resolve, join } from "node:path";
import { z } from "zod";
import { environmentHandler, bindingKey, accepted, unknown, failure, fail, EnvironmentError } from "../../../shared/environment-server.mjs";
export { serveEnvironment } from "../../../shared/environment-server.mjs";

const exec = promisify(execFile);
const configuration = z.strictObject({ profile: z.string().min(1) });
const profileSchema = z.strictObject({ image: z.string().min(1), workspace: z.enum(["read", "write"]),
  memoryMb: z.number().int().positive().default(256), pids: z.number().int().positive().default(64) });
const descriptor = z.discriminatedUnion("type", [
  z.strictObject({ type: z.literal("aex_official_tool"), version: z.literal(1), name: z.enum(["bash", "read", "write", "edit", "ls", "glob", "grep", "todo"]) }),
  z.strictObject({ type: z.literal("python_project"), name: z.string().regex(/^[A-Za-z0-9_-]+$/u) }),
]);

export async function createLocalEnvironment({ directory, profiles, docker = "docker" }) {
  const root = resolve(z.string().min(1).parse(directory));
  const configured = z.record(z.string(), profileSchema).parse(profiles);
  await mkdir(root, { recursive: true });
  const bindings = new Map();
  const running = new Map();
  const cli = async (...args) => (await exec(docker, args, { encoding: "utf8", maxBuffer: 20 * 1024 * 1024 })).stdout.trim();
  const filename = key => join(root, `${Buffer.from(key).toString("base64url")}.json`);
  async function save(key, state) {
    const temporary = `${filename(key)}.${randomUUID()}.tmp`;
    await writeFile(temporary, JSON.stringify(state), { flag: "wx", mode: 0o600 });
    await rename(temporary, filename(key));
  }
  async function load(key) {
    if (!bindings.has(key)) bindings.set(key, readFile(filename(key), "utf8").then(JSON.parse));
    return bindings.get(key);
  }
  const allocations = new Map();
  const containerName = (state, sequence) => `${state.volume.replace("aex-workspace-", "aex-call-")}-${sequence}`;
  async function allocate(key, state) {
    if (!allocations.has(key)) allocations.set(key, (async () => {
      if (state.phase === "ready") {
        return;
      }
      if (state.phase !== "new") fail("allocation_unknown", "workspace preparation was interrupted; operator inspection is required");
      state.phase = "allocating";
      await save(key, state);
      await cli("volume", "create", "--label", "aex.extension=env-local", state.volume);
      await cli("run", "--rm", "--network=none", "--cap-drop=ALL", "--cap-add=CHOWN", "--security-opt=no-new-privileges",
        "--mount", `type=volume,src=${state.volume},dst=/workspace,volume-nocopy`,
        "--entrypoint", "chown", state.profile.image, "1000:1000", "/workspace");
      state.phase = "ready";
      await save(key, state);
    })());
    await allocations.get(key);
    // The retained volume can disappear after an earlier invocation completed.
    try { await cli("volume", "inspect", state.volume); }
    catch { fail("resource_lost", "the retained workspace volume is missing; create a new binding explicitly"); }
  }
  async function execute(op, key, state) {
    const implementation = descriptor.safeParse(op.request.implementation);
    if (!implementation.success) fail("unsupported", "unsupported workspace implementation descriptor");
    const identity = `${key}/${op.sequence}`;
    if (running.has(identity)) fail("busy", "this invocation is already active");
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(new EnvironmentError("timeout", "execution deadline expired")), op.request.deadline_ms);
    const call = { controller, container: containerName(state, op.sequence), removal: undefined, created: false };
    running.set(identity, call);
    const remove = () => call.removal ??= cli("rm", "--force", call.container);
    const abort = () => { if (call.created) remove().catch(() => {}); };
    controller.signal.addEventListener("abort", abort, { once: true });
    let receipt;
    try {
      await allocate(key, state);
      controller.signal.throwIfAborted();
      const profile = state.profile;
      await cli("create", "--interactive", "--name", call.container, "--network=none", "--read-only",
        "--label", `aex.binding=${state.volume}`,
        "--cap-drop=ALL", "--security-opt=no-new-privileges", "--user=1000:1000",
        `--memory=${profile.memoryMb}m`, `--pids-limit=${profile.pids}`, "--tmpfs", "/tmp:rw,nosuid,nodev,size=32m",
        "--mount", `type=volume,src=${state.volume},dst=/workspace,volume-nocopy${profile.workspace === "read" ? ",readonly" : ""}`,
        profile.image);
      call.created = true;
      controller.signal.throwIfAborted();
      const child = spawn(docker, ["start", "--attach", "--interactive", call.container], { stdio: ["pipe", "pipe", "pipe"] });
      const completed = new Promise((resolveCall, reject) => {
        let stdout = "";
        let stderr = "";
        let bytes = 0;
        const collect = (which, chunk) => {
          bytes += Buffer.byteLength(chunk);
          if (bytes > 20 * 1024 * 1024) controller.abort(new EnvironmentError("output_limit", "execution output exceeded 20 MiB"));
          else if (which === "out") stdout += chunk;
          else stderr += chunk;
        };
        child.stdout.on("data", chunk => collect("out", chunk));
        child.stderr.on("data", chunk => collect("err", chunk));
        child.on("error", reject);
        child.stdin.on("error", reject);
        child.on("close", code => code === 0 ? resolveCall(stdout) : reject(new Error(`container exited ${code}: ${stderr}`)));
      });
      child.stdin.end(JSON.stringify({ implementation: implementation.data, input: op.request.input }));
      const output = await completed;
      controller.signal.throwIfAborted();
      receipt = JSON.parse(output);
      if (!["result", "failure"].includes(receipt.type)) receipt = unknown("workspace runner returned no terminal result");
    } catch (error) {
      if (controller.signal.aborted) receipt = failure(controller.signal.reason.code, controller.signal.reason.message);
      else if (call.created) receipt = unknown(error.message);
      else throw error;
    } finally {
      clearTimeout(timer);
      controller.signal.removeEventListener("abort", abort);
      try { if (call.created) await remove(); }
      catch (error) {
        if (receipt.type === "failure") receipt.details = { ...receipt.details, cleanup_error: error.message };
        else if (receipt.type === "unknown") receipt = unknown(`${receipt.message}; workspace cleanup failed: ${error.message}`);
        else receipt = failure("cleanup_failed", `workspace cleanup failed: ${error.message}`, { output: receipt.output });
      }
      finally { running.delete(identity); }
    }
    return receipt;
  }
  const handle = environmentHandler(async op => {
    const key = bindingKey(op);
    const request = op.request;
    if (request.type === "setup") {
      const { profile } = configuration.parse(request.configuration);
      if (!Object.hasOwn(configured, profile)) fail("unsupported", "unknown workspace profile");
      const image = await cli("image", "inspect", "--format", "{{.Id}}", configured[profile].image);
      const state = { profileName: profile, profile: { ...configured[profile], image }, phase: "new", volume: `aex-workspace-${randomUUID()}` };
      await writeFile(filename(key), JSON.stringify(state), { flag: "wx", mode: 0o600 });
      bindings.set(key, Promise.resolve(state));
      return accepted();
    }
    const state = await load(key);
    if (state.phase === "deleted") {
      if (request.type === "teardown") return accepted();
      fail("unavailable", "workspace binding was deleted");
    }
    if (request.type === "execute") return execute(op, key, state);
    if (request.type === "cancel") {
      const call = running.get(`${key}/${request.target_sequence}`);
      if (call) { call.controller.abort(new EnvironmentError("cancelled", "execution cancelled")); if (call.removal) await call.removal; }
      else {
        const name = containerName(state, request.target_sequence);
        const found = await cli("ps", "--all", "--filter", `name=^/${name}$`, "--filter", `label=aex.binding=${state.volume}`, "--format", "{{.Names}}");
        if (found) await cli("rm", "--force", name);
      }
      return accepted();
    }
    if (["detach", "teardown"].includes(request.type)) {
      if ([...running.keys()].some(id => id.startsWith(`${key}/`))) fail("busy", "workspace has active invocations");
      const abandoned = await cli("ps", "--all", "--filter", `label=aex.binding=${state.volume}`, "--format", "{{.ID}}");
      if (abandoned) await cli("rm", "--force", ...abandoned.split("\n"));
      if (request.type === "teardown") {
        if (state.phase !== "new") {
          const present = await cli("volume", "ls", "--filter", `name=^${state.volume}$`, "--format", "{{.Name}}");
          if (present) await cli("volume", "rm", state.volume);
        }
        state.phase = "deleted";
        await save(key, state);
      }
      return accepted();
    }
    fail("unsupported", "workspace lifecycle operation is not supported");
  });
  return { handle };
}
