import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";

import { app } from "./dist/index.mjs";
import { createEnvironment, serveEnvironment } from "./index.mjs";

test("publishes the application target as an ordinary Environment", () => {
  assert.deepEqual(Object.keys(app()), []);
});

test("runs the complete lifecycle with separate session authority", async () => {
  const calls = [];
  const handle = createEnvironment({ tools: { echo: async (input, context) => { calls.push(context); return input; } } });
  const setup = operation("op_setup", { type: "setup", configuration: { workspace: "main" } });
  assert.equal((await handle(setup)).receipt.type, "accepted");
  assert.deepEqual(await handle(setup), await handle(setup), "same operation and digest replays its receipt");
  assert.equal((await handle(operation("op_attach", { type: "attach", grants: { echo: true } }, "att_one"))).receipt.type, "accepted");
  const executed = await handle(operation("op_execute", {
    type: "execute",
    tool: { call_id: "call_one", name: "echo", input: { value: 1 } },
    remote_tool_id: "echo",
    tool_configuration: { prefix: "test" },
    grant: { echo: true },
  }, "att_one"));
  assert.deepEqual(executed.receipt.result.output, { value: 1 });
  assert.equal(calls[0].sessionId, "ses_one");
  assert.deepEqual(calls[0].options, { prefix: "test" });
  assert.equal((await handle(operation("op_detach", { type: "detach" }, "att_one"))).receipt.type, "accepted");
  assert.equal((await handle(operation("op_teardown", { type: "teardown" }))).receipt.type, "accepted");
});

test("rejects a conflicting replay and unauthenticated HTTP", async () => {
  const handle = createEnvironment();
  const first = operation("op_same", { type: "setup", configuration: {} });
  await handle(first);
  const conflict = structuredClone(first);
  conflict.operation.request_digest = "b".repeat(64);
  assert.equal((await handle(conflict)).receipt.type, "conflict");

  const service = serveEnvironment({ token: "secret" });
  const address = await service.listen(0);
  try {
    const response = await fetch(`http://127.0.0.1:${address.port}/v1/operations`, { method: "POST", body: "{}" });
    assert.equal(response.status, 401);
  } finally {
    await service.close();
  }
});

function operation(operationId, request, attachmentId) {
  const configuration = request.type === "setup" ? request.configuration : { workspace: "main" };
  return {
    contract: "environment/v1",
    binding: {
      environment_id: "env_shared",
      configuration_digest: digest(configuration),
      adapter_binding: JSON.stringify(configuration),
      directory_generation: 1,
      lifecycle_policy: "session",
    },
    operation: {
      operation_id: operationId,
      request_digest: "a".repeat(64),
      environment_id: "env_shared",
      session_id: "ses_one",
      ...(attachmentId === undefined ? {} : { attachment_id: attachmentId }),
      request,
    },
  };
}

function digest(value) {
  return createHash("sha256").update(canonical(value)).digest("hex");
}

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value !== null && typeof value === "object") return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  return JSON.stringify(value);
}
