import assert from "node:assert/strict";
import test from "node:test";
import { environmentHandler, serveEnvironment, accepted, finishExecution, result, deadlineTimer } from "./environment-server.mjs";
import { commands } from "./environment-test-fixture.mjs";

test("HTTP Environment authenticates and validates before invoking a driver", async t => {
  let calls = 0;
  const server = await serveEnvironment(environmentHandler(() => { calls++; return accepted(); }), { token: "fixture", maxBodyBytes: 1000 });
  t.after(server.close);
  const request = (body, token = "fixture", path = "/v1/operations") => fetch(server.url + path, {
    method: "POST", headers: { authorization: `Bearer ${token}` }, body: JSON.stringify(body),
  });
  const command = commands()("setup", { configuration: {} });
  assert.equal((await request(command, "wrong")).status, 401);
  assert.equal((await request(command, "fixture", "/elsewhere")).status, 404);
  assert.equal((await request({ ...command, contract: "environment/v9" })).status, 400);
  assert.equal(calls, 0);
  assert.equal((await request(command)).status, 200);
  assert.equal(calls, 1);
  assert.equal((await request({ payload: "x".repeat(1100) })).status, 413);
  assert.equal(calls, 1);
});

test("Tool execution requires a completion grant before any effects and accepts unlimited or long deadlines", async () => {
  let effects = 0;
  const handle = environmentHandler(() => { effects++; return accepted(); });
  const execute = commands()("execute", { implementation: {}, input: {} });
  const missing = structuredClone(execute);
  missing.operation.request.callback.methods = [];
  await assert.rejects(handle(missing), /completion service/u);
  assert.equal(effects, 0);
  await handle(execute);
  execute.operation.request.deadline_ms = 3_000_000_000;
  await handle(execute);
  assert.equal(effects, 2);
});

test("successful execution finishes with a durable acknowledgement and never retries a lost one", async () => {
  const operation = commands()("execute", { implementation: {}, input: {} }).operation;
  for (const [response, expected] of [[Response.json(12), "result"], [Response.json({}), "unknown"], [new Response("lost", { status: 503 }), "unknown"]]) {
    let calls = 0;
    const receipt = await finishExecution(operation, result({ saved: true }), async (_url, request) => {
      calls++;
      assert.deepEqual(JSON.parse(request.body), { method: "finish", input: { status: "ok", value: { saved: true } } });
      return response;
    });
    assert.equal(calls, 1);
    assert.equal(receipt.type, expected);
  }
});

test("deadline timers preserve the original long expiry and unlimited work has no timer", t => {
  t.mock.timers.enable({ apis: ["Date", "setTimeout"], now: 0 });
  let expired = 0;
  deadlineTimer(undefined, () => assert.fail("unlimited deadline fired"));
  deadlineTimer(3_000_000_000, () => expired++);
  t.mock.timers.tick(2_147_483_647);
  assert.equal(expired, 0);
  t.mock.timers.tick(3_000_000_000 - 2_147_483_647);
  assert.equal(expired, 1);
});
