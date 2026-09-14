import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { createModalEnvironment } from "../dist/server.mjs";

const profile = { image: "im-fixture", commands: { calculate: ["python", "/tools/calculate.py"] },
  cpu: 1, memoryMiB: 1024, region: "us", maxLifetimeMs: 2000, maxOutputBytes: 128 };
const command = (session_id, sequence, type, extra = {}) => ({ contract: "environment/v1",
  operation: { session_id, environment: "workspace", sequence, request: { type, ...extra } } });
const setup = (id = "first", extra = {}) => command(id, 1, "setup", { configuration: { profile: "cpu", lifetimeMs: 2000, ...extra } });
const execute = (id = "first", sequence = 2, input = {}) => command(id, sequence, "execute", {
  implementation: { type: "modal_command", name: "calculate" }, input, deadline_ms: 1900 });
async function fixture(t, options = {}) {
  const directory = await mkdtemp(join(tmpdir(), "aex-modal-test-"));
  const resources = new Map();
  const created = [];
  const executions = [];
  const reports = [];
  const client = { apps: { fromName: async () => ({ appId: "ap-fixture" }) }, images: { fromId: async imageId => ({ imageId }) },
    sandboxes: {
      async create(app, image, params) {
        created.push(params);
        const sandboxId = `sb-${created.length}`;
        let exit = null;
        let finish;
        const sandbox = { sandboxId, getTags: async () => params.tags, poll: async () => exit,
          terminate: async ({ wait }) => { assert.equal(wait, true); exit = 137; finish?.(); return exit; },
          exec: async (argv, opts) => {
            executions.push({ argv, opts });
            let output;
            const wait = new Promise(resolve => { finish = () => { output?.close(); resolve(137); }; });
            return {
              stdin: new WritableStream({ write(chunk) {
                const input = JSON.parse(chunk);
                if (!input.hang) { output.enqueue(JSON.stringify(input)); output.close(); finish = undefined; }
              } }),
              stdout: new ReadableStream({ start(controller) { output = controller; } }),
              stderr: new ReadableStream({ start(controller) { controller.close(); } }),
              wait: async () => options.hang ? wait : 0,
            };
          } };
        const original = sandbox.exec;
        sandbox.exec = async (...args) => { const child = await original(...args); child.stdin.writeText = async text => { const writer = child.stdin.getWriter(); await writer.write(text); writer.releaseLock(); }; return child; };
        resources.set(sandboxId, sandbox);
        if (options.lostCreate) throw new Error("provider response lost");
        return sandbox;
      },
      fromId: async id => { assert.ok(resources.has(id)); return resources.get(id); },
      fromName: async (app, name) => {
        const index = created.findIndex(params => params.name === name);
        if (index < 0 || options.hideResource) throw new Error("no running resource found");
        return resources.get(`sb-${index + 1}`);
      },
    } };
  let env;
  const open = async () => env = await createModalEnvironment({ directory, appName: "fixture", profiles: { cpu: profile }, client,
    report: async value => reports.push(value), ...options.hooks });
  await open();
  t.after(async () => { env.close(); await rm(directory, { recursive: true, force: true }); });
  return { get env() { return env; }, open, created, executions, reports, resources };
}

test("lazy allocation, shared binding, durable invocation identity and terminal usage", async t => {
  const f = await fixture(t);
  assert.equal((await f.env.handle(setup())).receipt.type, "accepted");
  assert.equal(f.created.length, 0);
  const calls = await Promise.all([f.env.handle(execute("first", 2, { value: 2 })), f.env.handle(execute("first", 3, { value: 3 }))]);
  assert.deepEqual(calls.map(item => item.receipt.output), [{ value: 2 }, { value: 3 }]);
  assert.equal(f.created.length, 1);
  assert.equal(f.created[0].blockNetwork, true);
  assert.equal(f.created[0].cpuLimit, 1);
  assert.equal(f.created[0].memoryLimitMiB, 1024);
  assert.equal(f.created[0].includeOidcIdentityToken, false);
  assert.equal(f.created[0].secrets, undefined);
  f.env.close(); await f.open();
  assert.equal((await f.env.handle(execute())).receipt.type, "unknown");
  assert.equal(f.executions.length, 2);
  assert.equal((await f.env.handle(execute("first", 4))).receipt.type, "result");
  assert.equal(f.created.length, 1);
  assert.equal((await f.env.handle(command("first", 5, "detach"))).receipt.type, "accepted");
  assert.equal((await f.resources.get("sb-1").poll()), 137);
  assert.equal((await f.env.handle(execute("first", 6))).receipt.code, "resource_lost");
  assert.equal(f.reports.at(-1).terminal, true);
  assert.ok(f.reports.at(-1).unitsMs <= 1000);
});

test("unknown allocation is never recreated after controller restart", async t => {
  const f = await fixture(t, { lostCreate: true, hideResource: true });
  await f.env.handle(setup());
  assert.equal((await f.env.handle(execute())).receipt.type, "unknown");
  f.env.close(); await f.open();
  assert.equal((await f.env.handle(execute("first", 3))).receipt.type, "unknown");
  assert.ok((await f.env.reconcile())[0].error);
  assert.equal((await f.env.handle(command("first", 4, "teardown"))).receipt.type, "unknown");
  assert.equal(f.created.length, 1);
  assert.equal(f.reports.some(report => report.terminal), false);
  await f.env.recover({ sessionId: "first", environment: "workspace", sandboxId: "sb-1" });
  assert.equal(f.reports.at(-1).terminal, true);
});

test("admission, immutable configuration and command catalog deny widening before allocation", async t => {
  let expiresAt;
  let active = true;
  const f = await fixture(t, { hooks: { authorize: async ({ configuration }) => {
    if (!active || configuration.authorization !== "approved") throw new Error("authorization denied");
    return expiresAt ??= Date.now() + 1600;
  } } });
  assert.equal((await f.env.handle(setup())).receipt.type, "failure");
  assert.equal((await f.env.handle(setup("first", { authorization: "approved" }))).receipt.type, "accepted");
  assert.equal((await f.env.handle(setup("first", { authorization: "approved", lifetimeMs: 900 }))).receipt.code, "conflict");
  const arbitrary = execute(); arbitrary.operation.request.implementation.name = "arbitrary";
  assert.equal((await f.env.handle(arbitrary)).receipt.code, "unsupported");
  const callback = execute(); callback.operation.request.callback = { url: "https://example.com", token: "privileged", methods: [] };
  // Brain offers invocation callbacks, but command processes receive only their JSON input.
  assert.deepEqual((await f.env.handle(callback)).receipt.output, {});
  assert.equal(f.executions[0].opts.env, undefined);
  active = false;
  assert.equal((await f.env.handle(execute("first", 3))).receipt.type, "failure");
  assert.equal(f.executions.length, 1);
  assert.equal((await f.env.handle(command("first", 4, "teardown"))).receipt.type, "accepted");
  assert.equal(f.reports.at(-1).terminal, true);
});

test("different bindings allocate different resources and expired bindings never allocate", async t => {
  const f = await fixture(t);
  await f.env.handle(setup()); await f.env.handle(setup("second"));
  await f.env.handle(execute()); await f.env.handle(execute("second"));
  assert.equal(f.created.length, 2);
  assert.notEqual(f.created[0].name, f.created[1].name);
  await f.env.handle(setup("expired", { lifetimeMs: 1 }));
  await new Promise(resolve => setTimeout(resolve, 5));
  assert.equal((await f.env.handle(execute("expired"))).receipt.code, "expired");
  assert.equal(f.created.length, 2);
  assert.equal(f.reports.at(-1).unitsMs, 0);
});

test("cancellation waits for resource termination and invalidates the shared binding", async t => {
  const f = await fixture(t, { hang: true });
  await f.env.handle(setup());
  const pending = f.env.handle(execute("first", 2, { hang: true }));
  while (!f.executions.length) await new Promise(resolve => setImmediate(resolve));
  assert.equal((await f.env.handle(command("first", 3, "cancel", { target_sequence: 2 }))).receipt.type, "accepted");
  assert.equal((await pending).receipt.code, "cancelled");
  assert.equal((await f.env.handle(execute("first", 4))).receipt.code, "resource_lost");
});

test("oversized output terminates compute before returning an output-limit failure", async t => {
  const f = await fixture(t);
  await f.env.handle(setup());
  assert.equal((await f.env.handle(execute("first", 2, { large: "x".repeat(200) }))).receipt.code, "output_limit");
  assert.equal(await f.resources.get("sb-1").poll(), 137);
});
