import assert from "node:assert/strict";
import { createServer } from "node:http";
import { join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

import { appTools } from "@aexhq/brain";
import { z } from "zod";

import { app } from "./dist/index.mjs";

test("publishes the application target as an ordinary Environment", () => {
  assert.deepEqual(Object.keys(app({ channelToken: "secret" })), []);
  assert.throws(() => app(), /expected object/u, "the configuration must carry the channel token");
  assert.throws(() => app({ channelToken: "" }), /Too small/u);
});

let sequence = 0;
const command = (request, attachmentId) => {
  sequence += 1;
  const id = `op_${sequence}`;
  return {
    contract: "environment/v2",
    binding: {},
    operation: { operation_id: id, request_identity: id.padEnd(64, "a"), environment_id: "env_app", session_id: "ses_test", ...(attachmentId === undefined ? {} : { attachment_id: attachmentId }), request },
  };
};

test("routes callback Tool invocations down the app's channel", async () => {
  const handle = (await import(pathToFileURL(join(import.meta.dirname, "dist/runtime/app.mjs")).href)).default;
  assert.equal((await handle(command({ type: "setup", configuration: { driver: "app", channelToken: "secret" } }))).receipt.type, "accepted");
  assert.equal((await handle(command({ type: "attach", grants: {}, provisions: [], bindings: {} }, "att_1"))).receipt.type, "accepted");

  const invoke = async (tool, input, deadlineMs = 5_000) => {
    sequence += 1;
    const invoked = await handle(command({ type: "invoke", call_id: `call_${sequence}`, tool, input, deadline_ms: deadlineMs }, "att_1"));
    assert.equal(invoked.receipt.type, "outcome", JSON.stringify(invoked.receipt));
    return invoked.receipt.outcome;
  };

  const unconnected = await invoke("create_invoice", { amount_cents: 100 });
  assert.equal(unconnected.status, "error");
  assert.equal(unconnected.error.code, "app_disconnected");

  const server = createServer();
  server.on("upgrade", (request, socket, head) => handle.channel.upgrade(request, socket, head));
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  const tools = appTools.connect({ url: `ws://127.0.0.1:${port}/environments/env_app/channel`, token: "secret" });
  try {
    tools.register({
      name: "create_invoice",
      description: "Create an invoice in this app.",
      input: z.object({ amount_cents: z.number().int() }),
    }, async (input) => ({ id: "inv_1", amount_cents: input.amount_cents }));
    tools.register({
      name: "hang",
      description: "Never answers until cancelled.",
      input: z.object({}),
    }, (input, call) => new Promise((_, reject) => call.signal.addEventListener("abort", () => reject(new Error("aborted")))));
    await tools.ready();

    assert.deepEqual(await invoke("create_invoice", { amount_cents: 250 }), { status: "ok", value: { id: "inv_1", amount_cents: 250 } });
    const unknown = await invoke("missing_tool", {});
    assert.equal(unknown.status, "error");
    assert.equal(unknown.error.code, "unknown_tool");
    assert.deepEqual(await invoke("hang", {}, 100), { status: "timeout" }, "the environment owns the deadline and cancels down the channel");
  } finally {
    tools.close();
    await new Promise((resolve) => server.close(resolve));
  }
  assert.equal((await handle(command({ type: "detach" }, "att_1"))).receipt.type, "accepted");
  assert.equal((await handle(command({ type: "teardown" }))).receipt.type, "accepted");
});
