import assert from "node:assert/strict";
import test from "node:test";
import { environmentHandler, serveEnvironment, accepted } from "./environment-server.mjs";
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
