import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdtemp, rm, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ModalClient } from "modal";
import { createModalClient, createModalEnvironment, serveEnvironment } from "../dist/server.mjs";
import { completionGrant, finishCallback } from "../../../shared/environment-test-fixture.mjs";

const python = `import json,os,socket,sys,time,subprocess
from pathlib import Path
packet=json.load(sys.stdin)
data=packet["input"]
op=data["op"]
if op=="write":
 Path("value").write_text(data["value"])
 print(json.dumps({"written":True}))
elif op=="read":
 print(json.dumps({"value":Path("value").read_text() if Path("value").exists() else None}))
elif op=="probe":
 denied=[]
 for address in ["1.1.1.1","169.254.169.254"]:
  try:
   socket.create_connection((address,80),timeout=1).close()
   denied.append(False)
  except OSError: denied.append(True)
 keys=["MODAL_TOKEN_ID","MODAL_TOKEN_SECRET","MODAL_IDENTITY_TOKEN","AWS_ACCESS_KEY_ID","AWS_SECRET_ACCESS_KEY","AEX_API_KEY","SUPABASE_SECRET_KEY"]
 print(json.dumps({"uid":os.getuid(),"network_denied":denied,"privileged_credentials":any(key in os.environ for key in keys),"configuration":packet["configuration"],"invocation":packet["invocation"]}))
elif op=="hang":
 subprocess.Popen([sys.executable,"-c","import time; from pathlib import Path; time.sleep(45); Path('late').write_text('side effect')"])
 Path("started").write_text("yes")
 time.sleep(60)
elif op=="burst": print(json.dumps({"text":"x"*100000}))
else: raise ValueError("unknown operation")`;

async function integration(t) {
  assert.ok(process.env.MODAL_TOKEN_ID && process.env.MODAL_TOKEN_SECRET, "Modal credentials are required");
  const credentials = { tokenId: process.env.MODAL_TOKEN_ID, tokenSecret: process.env.MODAL_TOKEN_SECRET };
  const client = createModalClient(credentials);
  const appName = "aex-environment-integration";
  const builder = new ModalClient(credentials);
  let image;
  try {
    const app = await builder.apps.fromName(appName, { createIfMissing: true });
    const fixture = new URL("../../../test/fixtures/package-tool/", import.meta.url);
    const files = await Promise.all(["package.json", "runtime.mjs"].map(async name =>
      `RUN printf '%s' '${(await readFile(new URL(name, fixture))).toString("base64")}' | base64 -d > /opt/runtime/node_modules/@fixture/package-tool/${name}`));
    image = await builder.images.fromRegistry("node:22.23.2-bookworm-slim")
      .dockerfileCommands(["RUN apt-get update && apt-get install -y --no-install-recommends python3 && rm -rf /var/lib/apt/lists/*",
        "RUN npm install --prefix /opt/runtime --ignore-scripts @aexhq/brain@0.31.0 zod@4.4.3 && mkdir -p /opt/runtime/node_modules/@fixture/package-tool",
        ...files, "RUN mkdir -p /workspace && chown 1000:1000 /workspace", "WORKDIR /workspace", "USER 1000:1000"]).build(app);
  } finally { await builder.close(); }
  const directory = await mkdtemp(join(tmpdir(), "aex-modal-integration-"));
  const profiles = { cpu: { image: image.imageId, commands: { fixture: ["python3", "-c", python] },
    toolRuntime: ["node", "/opt/runtime/node_modules/@aexhq/brain/bin/brain-tool-runtime.mjs", "/opt/runtime"],
    cpu: 1, memoryMiB: 1024, maxLifetimeMs: 90_000, region: "us", maxOutputBytes: 8192, terminateAfterTurn: true } };
  const reports = [];
  const services = [];
  let env;
  let server;
  const token = randomUUID();
  const open = async () => {
    env = await createModalEnvironment({ directory, appName, profiles, client, fetch: async (url, request) => {
      const call = JSON.parse(request.body); services.push(call);
      assert.equal(request.headers.authorization, "Bearer fixture");
      if (call.method === "finish") return finishCallback(url, request);
      if (call.method === "model") return Response.json({ message: { role: "assistant", content: [{ type: "text", text: "Summary" }] }, stop_reason: "end_turn", usage: {} });
      assert.equal(call.method, "result");
      return Response.json(100);
    }, report: async usage => reports.push(usage) });
    server = await serveEnvironment(env.handle, { token });
  };
  await open();
  const sessions = [randomUUID(), randomUUID(), randomUUID(), randomUUID()];
  const sequences = new Map();
  const call = async (session_id, type, extra = {}) => {
    const sequence = (sequences.get(session_id) ?? 0) + 1;
    sequences.set(session_id, sequence);
    const response = await fetch(`${server.url}/v1/operations`, { method: "POST", headers: { authorization: `Bearer ${token}` },
      body: JSON.stringify({ contract: "environment/v1", operation: { session_id, environment: "cpu", sequence, request: { type, ...extra } } }) });
    assert.equal(response.status, 200);
    return (await response.json()).receipt;
  };
  const execute = (id, input, deadline_ms = 30_000) => call(id, "execute", { implementation: { type: "modal_command", name: "fixture", configuration: { scope: "one-run" } }, input, deadline_ms, callback: completionGrant });
  const started = Date.now();
  t.after(async () => {
    try {
      for (const id of sessions.filter(id => sequences.has(id))) {
        const receipt = await call(id, "teardown");
        assert.equal(receipt.type, "accepted", JSON.stringify(receipt));
      }
      await server.close(); env.close();
      await rm(directory, { recursive: true, force: true });
    } finally { client.close(); }
  });
  assert.equal((await fetch(`${server.url}/v1/operations`, { method: "POST" })).status, 401);
  for (const id of sessions.slice(0, 2)) assert.equal((await call(id, "setup", { configuration: { profile: "cpu", lifetimeMs: 90_000 } })).type, "accepted");
  assert.equal(reports.length, 0);
  const write = await execute(sessions[0], { op: "write", value: "retained" });
  assert.equal(write.type, "result", JSON.stringify(write));
  services.length = 0;
  const packaged = await call(sessions[0], "execute", { implementation: { type: "node_package", package: "@fixture/package-tool", version: "1.0.0", entry: "./runtime", export: "report" },
    input: { path: "value" }, deadline_ms: 30_000, callback: { ...completionGrant, methods: ["model", "result", "returned", "finish"] } });
  assert.equal(packaged.type, "returned", JSON.stringify(packaged));
  assert.deepEqual(services.map(value => value.method), ["result", "model", "model", "finish"]);
  assert.equal(services.at(-1).input.value.document, "retained");
  assert.equal(services.at(-1).input.content, "Summarized the file.");
  assert.deepEqual((await execute(sessions[0], { op: "probe" })).output,
    { uid: 1000, network_denied: [true, true], privileged_credentials: false, configuration: { scope: "one-run" },
      invocation: { sessionId: sessions[0], environment: "cpu", sequence: sequences.get(sessions[0]) } });
  assert.deepEqual((await execute(sessions[1], { op: "read" })).output, { value: null });
  await server.close(); env.close(); await open();
  assert.deepEqual((await execute(sessions[0], { op: "read" })).output, { value: "retained" });
  assert.equal(new Set(reports.filter(item => item.sessionId === sessions[0]).map(item => item.sandboxId)).size, 1);
  const sandboxId = reports.find(item => item.sessionId === sessions[0]).sandboxId;
  const sandbox = await client.sandboxes.fromId(sandboxId);
  const pending = execute(sessions[0], { op: "hang" }, 60_000);
  const target_sequence = sequences.get(sessions[0]);
  const deadline = Date.now() + 20_000;
  for (;;) {
    try { if (await sandbox.filesystem.readText("/workspace/started") === "yes") break; } catch {}
    assert.ok(Date.now() < deadline, "tool failed to start before cancellation");
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  assert.equal((await call(sessions[0], "cancel", { target_sequence })).type, "accepted");
  assert.equal((await pending).code, "cancelled");
  assert.notEqual(await sandbox.poll(), null);
  assert.equal((await execute(sessions[0], { op: "read" })).code, "resource_lost");
  assert.equal((await execute(sessions[1], { op: "burst" })).code, "output_limit");
  assert.equal((await call(sessions[2], "setup", { configuration: { profile: "cpu", lifetimeMs: 30_000 } })).type, "accepted");
  const finite = await execute(sessions[2], { op: "read" });
  assert.equal(finite.type, "result", JSON.stringify(finite));
  const expiring = await client.sandboxes.fromId(reports.find(item => item.sessionId === sessions[2]).sandboxId);
  assert.notEqual(await expiring.wait(), null);
  assert.equal((await env.reconcile()).some(item => item.error), false);
  assert.equal((await execute(sessions[2], { op: "read" })).code, "resource_lost");
  assert.deepEqual(await call(sessions[3], "setup", { configuration: { profile: "cpu", lifetimeMs: 30_000 } }),
    { type: "accepted", on_turn_end: "terminate" });
  assert.equal((await execute(sessions[3], { op: "read" })).type, "result");
  assert.deepEqual(await call(sessions[3], "call", { name: "terminate", input: { sequence: 1 } }), { type: "result", output: null });
  const completed = await client.sandboxes.fromId(reports.find(item => item.sessionId === sessions[3]).sandboxId);
  assert.notEqual(await completed.poll(), null);
  assert.equal((await execute(sessions[3], { op: "read" })).code, "resource_lost");
  const finals = reports.filter(item => item.terminal);
  assert.equal(new Set(finals.map(item => item.sessionId)).size, 4);
  assert.ok(finals.every(item => item.unitsMs >= 0 && item.unitsMs <= 90_000));
  t.diagnostic(JSON.stringify({ image: image.imageId, elapsedMs: Date.now() - started,
    resources: [...new Set(finals.map(item => item.sandboxId))], usage: finals.map(({ sessionId, unitsMs }) => ({ sessionId, unitsMs })) }));
}

test("real Modal: shared workspace, isolation, restart, network, cancellation and finite lifetime", { timeout: 240_000 }, async t => {
  try { await integration(t); }
  catch (error) { console.error(error); throw error; }
});
