import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer } from "node:http";
import { once } from "node:events";
import { local } from "../dist/index.mjs";
import { createLocalEnvironment, serveEnvironment } from "../dist/server.mjs";
import { write, read, edit, bash } from "../../tools/dist/index.js";
import { codex } from "../../loop-codex/dist/index.mjs";
import { fixture, calls, answer, collect } from "../../../shared/journey-fixture.mjs";

async function workspace(t) {
  const directory = await mkdtemp(join(tmpdir(), "aex-local-journey-"));
  const runtime = await createLocalEnvironment({ directory, profiles: {
    coding: { image: process.env.BRAIN_WORKSPACE_IMAGE ?? "aex-workspace:test", workspace: "write" },
  } });
  t.after(() => rm(directory, { recursive: true }));
  return runtime;
}

test("Local Environment: edit and test a real program, then read it in a later turn", { timeout: 90_000 }, async t => {
  const f = await fixture(t, [
    () => calls(["write", { path: "app.mjs", content: "console.log(1 + 1)" }]),
    () => calls(["edit", { path: "app.mjs", old_text: "1 + 1", new_text: "1 + 2" }]),
    () => calls(["bash", { command: "node app.mjs" }]),
    body => { assert.match(body.messages.at(-1).content, /3\\n/u); return answer("verified"); },
    () => calls(["read", { path: "app.mjs", offset: 0, limit: 262144 }]),
    body => { assert.match(body.messages.at(-1).content, /1 \+ 2/u); return answer("retained"); },
  ]);
  const runtime = await workspace(t);
  const server = await serveEnvironment(runtime.handle, { token: "local-fixture" });
  t.after(server.close);
  const env = local({ name: "workspace", url: server.url, token: "local-fixture", profile: "coding" });
  const session = await f.session(codex, { tools: [write, read, edit, bash].map(factory => factory({ env })) });
  assert.equal((await session.send("edit and test")).status, "idle");
  assert.equal((await session.send("read it again")).status, "idle");
  const events = await collect(session.events());
  assert.equal(events.filter(event => event.type === "tool_call_started").length, 4);
  assert.equal(events.filter(event => event.type === "tool_call_ended").length, 4);
});

test("Local Environment: losing an HTTP response after a write produces unknown without replay", { timeout: 60_000 }, async t => {
  const f = await fixture(t, [
    () => calls(["write", { path: "once", content: "committed externally" }]),
    body => { assert.match(body.messages.at(-1).content, /unknown/u); return calls(["read", { path: "once", offset: 0, limit: 262144 }]); },
    body => { assert.match(body.messages.at(-1).content, /committed externally/u); return answer("inspected uncertain write"); },
  ]);
  const runtime = await workspace(t);
  let writes = 0;
  const server = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const command = JSON.parse(Buffer.concat(chunks).toString());
    const outcome = await runtime.handle(command);
    if (command.operation.request.implementation?.name === "write") { writes++; request.socket.destroy(); }
    else response.end(JSON.stringify(outcome));
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  t.after(() => new Promise(resolve => server.close(resolve)));
  const env = local({ name: "workspace", url: `http://127.0.0.1:${server.address().port}`, token: "fixture", profile: "coding" });
  const session = await f.session(codex, { tools: [write({ env }), read({ env })] });
  await session.send("write once");
  assert.equal(writes, 1);
  const events = await collect(session.events());
  assert.ok(events.some(event => event.type === "tool_call_ended" && JSON.stringify(event.data).includes("unknown")));
});
