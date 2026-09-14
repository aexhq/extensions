import { DatabaseSync } from "node:sqlite";
import { mkdir } from "node:fs/promises";
import { resolve, join } from "node:path";
import { randomUUID } from "node:crypto";
import { ModalClient, InvalidError } from "modal";
import { z } from "zod";
import { environmentHandler, bindingKey, identifier, accepted, result, unknown, failure, fail, EnvironmentError } from "../../../shared/environment-server.mjs";
export { serveEnvironment } from "../../../shared/environment-server.mjs";

const configuration = z.strictObject({ profile: identifier, lifetimeMs: z.number().int().positive().max(86_400_000),
  authorization: z.string().min(1).optional() });
const profileSchema = z.strictObject({ image: z.string().regex(/^im-[A-Za-z0-9]+$/u),
  commands: z.record(identifier, z.array(z.string().min(1)).min(1)),
  cpu: z.number().positive(), memoryMiB: z.number().int().positive(),
  maxLifetimeMs: z.number().int().positive().max(86_400_000),
  workdir: z.string().startsWith("/").default("/workspace"), region: z.string().min(1),
  outboundDomains: z.array(z.string().regex(/^(\*\.)?[a-z0-9-]+(?:\.[a-z0-9-]+)+$/u)).default([]),
  maxOutputBytes: z.number().int().positive().max(16 * 1024 * 1024).default(4 * 1024 * 1024) });
const descriptor = z.strictObject({ type: z.literal("modal_command"), name: identifier });
const unprivileged = ["/usr/bin/setpriv", "--reuid=1000", "--regid=1000", "--clear-groups", "--no-new-privs",
  "--inh-caps=-all", "--bounding-set=-all", "--ambient-caps=-all", "--"];

export function createModalClient(options = {}) {
  // Modal 0.10.1 does not apply constructor maxRetries/timeoutMs to its gRPC clients.
  return new ModalClient({ ...options, maxThrottleWaitSecs: 0, grpcMiddleware: [async function* (call, options) {
    return yield* call.next(call.request, { ...options, retries: 0, timeoutMs: options.timeoutMs ?? 30_000 });
  }] });
}

export async function createModalEnvironment({ directory, appName, profiles, client,
  authorize, report = async () => {} }) {
  const root = resolve(z.string().min(1).parse(directory));
  const configured = z.record(identifier, profileSchema).parse(profiles);
  z.string().min(1).parse(appName);
  await mkdir(root, { recursive: true });
  const modal = client ?? createModalClient();
  if (modal.profile?.sandboxV2) throw new Error("env-modal requires the stable Sandbox backend; disable MODAL_SANDBOX_V2");
  const app = await modal.apps.fromName(appName);
  const db = new DatabaseSync(join(root, "modal.sqlite"));
  db.exec(`PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
    CREATE TABLE IF NOT EXISTS bindings (id TEXT PRIMARY KEY, document TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS invocations (binding TEXT NOT NULL REFERENCES bindings(id), sequence INTEGER NOT NULL,
      PRIMARY KEY(binding,sequence));`);
  const states = new Map(db.prepare("SELECT id,document FROM bindings").all().map(row => [row.id, JSON.parse(row.document)]));
  const allocations = new Map();
  const stops = new Map();
  const running = new Map();
  const save = state => db.prepare("UPDATE bindings SET document=? WHERE id=?").run(JSON.stringify(state), state.key);
  const binding = key => states.get(key) ?? fail("unavailable", "unknown Modal binding");
  const tags = state => ({ "aex.binding": state.name });
  const units = state => state.startedAt === null ? 0 : Math.max(0, Math.min(state.stoppedAt ?? Date.now(), state.expiresAt) - state.startedAt);
  async function publish(state) {
    const unitsMs = units(state);
    const terminal = state.phase === "stopped";
    await report({ sessionId: state.sessionId, environment: state.environment, authorization: state.configuration.authorization,
      profile: state.configuration.profile, sandboxId: state.sandboxId, unitsMs, terminal });
  }
  async function ended(state) {
    state.phase = "stopped";
    state.stoppedAt ??= Date.now();
    save(state);
    await publish(state);
  }
  async function resource(state) {
    if (!state.sandboxId) {
      // A missing running name is not proof that an interrupted create had no effect.
      const found = await modal.sandboxes.fromName(appName, state.name);
      if ((await found.getTags())["aex.binding"] !== state.name) fail("ownership", "Modal resource ownership does not match");
      state.sandboxId = found.sandboxId;
      save(state);
    }
    return modal.sandboxes.fromId(state.sandboxId);
  }
  async function stop(state, reason = "cancelled") {
    if (stops.has(state.key)) return stops.get(state.key);
    const pending = (async () => {
      if (state.phase === "stopped") { await publish(state); return; }
      if (state.phase === "new") { await ended(state); return; }
      state.phase = "stopping";
      state.stopReason = reason;
      save(state);
      try { await allocations.get(state.key); } catch { /* Resolve the original resource below, never recreate it. */ }
      if (state.startedAt === null) { await ended(state); return; }
      const sandbox = await resource(state);
      await sandbox.terminate({ wait: true });
      await ended(state);
    })();
    stops.set(state.key, pending);
    try { await pending; } finally { stops.delete(state.key); }
  }
  async function allocate(state) {
    if (state.phase === "new" && !allocations.has(state.key)) {
      // Commit identity before the provider call. This record survives a lost create response.
      state.phase = "allocating";
      save(state);
      const pending = (async () => {
        let image;
        try { image = await modal.images.fromId(state.profile.image); }
        catch (error) { await ended(state); throw error; }
        if (state.phase !== "allocating") fail("cancelled", "binding was stopped before allocation");
        // Modal accepts whole seconds; round down so provider lifetime cannot widen the grant.
        const timeoutMs = Math.floor((state.expiresAt - Date.now()) / 1000) * 1000;
        if (timeoutMs <= 0) { await ended(state); fail("expired", "binding lifetime expired before allocation"); }
        state.startedAt = Date.now();
        save(state);
        const p = state.profile;
        let sandbox;
        try { sandbox = await modal.sandboxes.create(app, image, { name: state.name, tags: tags(state),
          cpu: p.cpu, cpuLimit: p.cpu, memoryMiB: p.memoryMiB, memoryLimitMiB: p.memoryMiB,
          timeoutMs, workdir: p.workdir, regions: [p.region], includeOidcIdentityToken: false,
          ...(p.outboundDomains.length === 0 ? { blockNetwork: true } : { outboundDomainAllowlist: p.outboundDomains }) }); }
        catch (error) {
          if (error instanceof InvalidError || error.code === 3) { state.startedAt = null; await ended(state); }
          throw error;
        }
        state.sandboxId = sandbox.sandboxId;
        if (state.phase === "allocating") state.phase = "ready";
        save(state);
      })();
      allocations.set(state.key, pending);
    }
    if (allocations.has(state.key)) await allocations.get(state.key);
    if (state.phase === "allocating") fail("allocation_unknown", "allocation was interrupted; reconcile the original resource before further work");
    if (state.phase !== "ready") fail("resource_lost", "this binding is no longer executable; create a new binding explicitly");
    const sandbox = await resource(state);
    if (await sandbox.poll() !== null) {
      await ended(state);
      fail("resource_lost", "the original Modal Sandbox has stopped");
    }
    if (state.expiresAt <= Date.now()) {
      await stop(state, "expired");
      fail("expired", "binding lifetime expired");
    }
    return sandbox;
  }
  async function execute(op, state) {
    const parsed = descriptor.safeParse(op.request.implementation);
    if (!parsed.success || !Object.hasOwn(state.profile.commands, parsed.data.name)) fail("unsupported", "unknown Modal command");
    if (db.prepare("SELECT 1 FROM invocations WHERE binding=? AND sequence=?").get(state.key, op.sequence)) {
      return unknown("invocation sequence was already accepted; consult the Brain journal, it will not be replayed");
    }
    if (["stopped", "stopping"].includes(state.phase)) fail("resource_lost", "this binding is no longer executable; create a new binding explicitly");
    db.prepare("INSERT INTO invocations VALUES(?,?)").run(state.key, op.sequence);
    const identity = `${state.key}/${op.sequence}`;
    running.set(identity, true);
    let timer;
    let dispatched = false;
    let receipt;
    let timeoutStop;
    const deadline = Date.now() + op.request.deadline_ms;
    try {
      if (authorize) {
        const expiresAt = await authorize({ sessionId: state.sessionId, environment: state.environment, configuration: state.configuration });
        if (expiresAt !== state.expiresAt) fail("authorization_changed", "binding lifetime authorization is immutable");
      }
      const sandbox = await allocate(state);
      const remaining = Math.floor((Math.min(deadline, state.expiresAt) - Date.now()) / 1000) * 1000;
      if (remaining <= 0) fail("timeout", "invocation deadline expired before dispatch");
      await publish(state);
      timer = setTimeout(() => { timeoutStop = stop(state, "timeout"); timeoutStop.catch(() => {}); }, remaining);
      dispatched = true;
      const child = await sandbox.exec([...unprivileged, ...state.profile.commands[parsed.data.name]], { mode: "text", timeoutMs: remaining, workdir: state.profile.workdir });
      let bytes = 0;
      const read = async stream => {
        let output = "";
        for await (const chunk of stream) {
          bytes += Buffer.byteLength(chunk);
          if (bytes > state.profile.maxOutputBytes) {
            await stop(state, "output_limit");
            fail("output_limit", "Modal command exceeded its output limit");
          }
          output += chunk;
        }
        return output;
      };
      const write = async () => { await child.stdin.writeText(JSON.stringify(op.request.input)); await child.stdin.close(); };
      const [, stdout, stderr, exitCode] = await Promise.all([write(), read(child.stdout), read(child.stderr), child.wait()]);
      if (state.stopReason) receipt = failure(state.stopReason, "Modal Sandbox was terminated with its active invocations");
      else if (exitCode !== 0) receipt = failure("command_failed", `Modal command exited ${exitCode}: ${stderr.slice(0, 2048)}`);
      else receipt = result(JSON.parse(stdout));
    } catch (error) {
      if (state.phase === "stopped" && state.stopReason) receipt = failure(state.stopReason, "Modal Sandbox was terminated with its active invocations");
      else if (dispatched || state.phase === "allocating" || state.phase === "stopping") receipt = unknown(error.message);
      else receipt = failure(error instanceof EnvironmentError ? error.code : "environment_failed", error.message);
    } finally {
      clearTimeout(timer);
      if (timeoutStop) { try { await timeoutStop; } catch (error) { receipt = unknown(`sandbox termination could not be confirmed: ${error.message}`); } }
      running.delete(identity);
    }
    return receipt;
  }
  const handle = environmentHandler(async op => {
    const key = bindingKey(op);
    const request = op.request;
    if (request.type === "setup") {
      const config = configuration.parse(request.configuration);
      const existing = states.get(key);
      if (existing) {
        if (JSON.stringify(existing.configuration) !== JSON.stringify(config)) fail("conflict", "binding configuration is immutable");
        return existing.phase === "stopped" ? failure("resource_lost", "binding has stopped") : accepted();
      }
      if (!Object.hasOwn(configured, config.profile)) fail("unsupported", "unknown Modal profile");
      const profile = configured[config.profile];
      if (config.lifetimeMs > profile.maxLifetimeMs) fail("budget", "requested lifetime exceeds the profile limit");
      const now = Date.now();
      const expiresAt = authorize ? await authorize({ sessionId: op.session_id, environment: op.environment, configuration: config }) : now + config.lifetimeMs;
      if (!Number.isSafeInteger(expiresAt) || expiresAt <= now || expiresAt > now + config.lifetimeMs) fail("budget", "invalid or expired lifetime authorization");
      const state = { key, sessionId: op.session_id, environment: op.environment, configuration: config,
        profile, name: `aex-${randomUUID()}`, phase: "new", expiresAt, startedAt: null, stoppedAt: null, sandboxId: null };
      db.prepare("INSERT INTO bindings VALUES(?,?)").run(key, JSON.stringify(state));
      states.set(key, state);
      return accepted();
    }
    const state = binding(key);
    if (request.type === "execute") return execute(op, state);
    if (["cancel", "detach", "teardown"].includes(request.type)) {
      if (request.type === "cancel" && !db.prepare("SELECT 1 FROM invocations WHERE binding=? AND sequence=?").get(key, request.target_sequence)) return accepted();
      try { await stop(state); return accepted(); }
      catch (error) { return unknown(`sandbox termination could not be confirmed: ${error.message}`); }
    }
    fail("unsupported", "Modal Environment calls are not supported");
  });
  return {
    handle,
    async reconcile() {
      const observations = [];
      for (const state of states.values()) {
        try {
          if (state.phase === "stopping" || state.expiresAt <= Date.now()) await stop(state, "expired");
          else if (["allocating", "ready"].includes(state.phase)) {
            const sandbox = await resource(state);
            if (await sandbox.poll() !== null) await ended(state);
            else { if (state.phase === "allocating") { state.phase = "ready"; save(state); } await publish(state); }
          } else if (state.phase === "stopped") await publish(state);
          observations.push({ binding: state.key, phase: state.phase });
        } catch (error) { observations.push({ binding: state.key, phase: state.phase, error: String(error.message) }); }
      }
      return observations;
    },
    async recover({ sessionId, environment, sandboxId }) {
      const state = binding(`${sessionId}/${environment}`);
      if (state.sandboxId && state.sandboxId !== sandboxId) fail("conflict", "binding already names another resource");
      const sandbox = await modal.sandboxes.fromId(sandboxId);
      if ((await sandbox.getTags())["aex.binding"] !== state.name) fail("ownership", "Modal resource ownership does not match");
      state.sandboxId = sandboxId;
      save(state);
      await stop(state);
    },
    close() {
      if (running.size || stops.size) throw new Error("Modal controller has active requests");
      db.close();
      if (!client) modal.close();
    },
  };
}
