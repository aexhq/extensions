import assert from "node:assert/strict";
import test from "node:test";
import { createBrowserEnvironment } from "../dist/server.mjs";
import { commands, eventually } from "../../../shared/environment-test-fixture.mjs";
import { browserFixture } from "./fixture.mjs";

const invocation = (name, input = {}, deadline_ms = 10_000) => ({ implementation: { type: "aex_browser_tool", version: 1, name: `browser_${name}` }, input, deadline_ms });

test("real browser state is retained, isolated, serialized and rendered as image media", { timeout: 45_000 }, async t => {
  const { env, url, launched } = await browserFixture(t);
  const first = commands();
  const other = commands();
  for (const command of [first, other]) assert.equal((await env.handle(command("setup", { configuration: { profile: "test" } }))).receipt.type, "accepted");
  assert.equal(launched.length, 0);
  const navigation = (await env.handle(first("execute", invocation("navigate", { url })))).receipt;
  assert.equal(navigation.type, "result", JSON.stringify(navigation));
  const results = await Promise.all([
    env.handle(first("execute", invocation("fill", { selector: "#name", value: "Ada" }))),
    env.handle(first("execute", invocation("click", { selector: "#save" }))),
  ]);
  assert.ok(results.every(value => value.receipt.type === "result"));
  assert.match((await env.handle(first("execute", invocation("inspect")))).receipt.output.snapshot, /Ada/u);
  await env.handle(first("detach"));
  assert.match((await env.handle(first("execute", invocation("inspect")))).receipt.output.snapshot, /Ada/u);
  assert.equal(launched.length, 1);
  const screenshot = (await env.handle(first("execute", invocation("screenshot")))).receipt.output;
  assert.equal(screenshot.type, "aex_tool_output");
  const png = Buffer.from(screenshot.media[0].url.split(",")[1], "base64");
  assert.equal(png.subarray(1, 4).toString(), "PNG");
  await env.handle(other("execute", invocation("navigate", { url })));
  assert.equal(await launched[1].contexts()[0].pages()[0].evaluate(() => localStorage.getItem("name")), null);
  assert.equal((await env.handle(first("execute", invocation("navigate", { url: "file:///etc/passwd" })))).receipt.type, "failure");
  await launched[0].close();
  assert.equal((await env.handle(first("execute", invocation("inspect")))).receipt.code, "resource_lost");
  assert.equal(launched.length, 2);
  for (const command of [first, other]) assert.equal((await env.handle(command("teardown"))).receipt.type, "accepted");
  assert.equal((await env.handle(other("teardown"))).receipt.type, "accepted");
  assert.ok(launched.every(browser => !browser.isConnected()));
});

test("cancellation closes the active browser and a restarted controller reports resource loss", { timeout: 30_000 }, async t => {
  const { env, url, launched } = await browserFixture(t);
  const command = commands();
  await env.handle(command("setup", { configuration: { profile: "test" } }));
  const navigation = (await env.handle(command("execute", invocation("navigate", { url })))).receipt;
  assert.equal(navigation.type, "result", JSON.stringify(navigation));
  const pending = command("execute", invocation("click", { selector: "#missing" }, 20_000));
  const running = env.handle(pending);
  await eventually(async () => launched[0].isConnected());
  await new Promise(resolve => setImmediate(resolve));
  await env.handle(command("cancel", { target_sequence: pending.operation.sequence }));
  const cancelled = (await running).receipt;
  assert.equal(cancelled.type, "failure");
  assert.equal(cancelled.code, "cancelled");
  assert.equal(launched[0].isConnected(), false);
  assert.equal((await env.handle(command("execute", invocation("inspect")))).receipt.code, "resource_lost");
  const restarted = createBrowserEnvironment({ profiles: { test: () => { throw new Error("must not replace a lost browser"); } } });
  assert.equal((await restarted.handle(command("execute", invocation("inspect")))).receipt.code, "resource_lost");
});

test("a Playwright navigation timeout closes uncertain browser state before the caller deadline", { timeout: 30_000 }, async t => {
  const { env, url, launched } = await browserFixture(t);
  const command = commands();
  await env.handle(command("setup", { configuration: { profile: "test" } }));
  assert.equal((await env.handle(command("execute", invocation("navigate", { url })))).receipt.type, "result");
  launched[0].contexts()[0].pages()[0].setDefaultNavigationTimeout(100);
  const receipt = (await env.handle(command("execute", invocation("navigate", { url: `${url}/hanging` }, 10_000)))).receipt;
  assert.equal(receipt.type, "failure");
  assert.equal(receipt.code, "timeout");
  assert.match(receipt.message, /Timeout 100ms/u);
  assert.equal(launched[0].isConnected(), false);
  assert.equal((await env.handle(command("execute", invocation("inspect")))).receipt.code, "resource_lost");
  assert.equal(launched.length, 1);
});

test("browser driver deadline retains its cause when cleanup fails", { timeout: 30_000 }, async t => {
  const { env, url, launched } = await browserFixture(t);
  const command = commands();
  await env.handle(command("setup", { configuration: { profile: "test" } }));
  await env.handle(command("execute", invocation("navigate", { url })));
  const browser = launched[0];
  const close = browser.close.bind(browser);
  browser.close = async () => { await close(); throw new Error("fixture cleanup failure"); };
  const receipt = (await env.handle(command("execute", invocation("click", { selector: "#missing" }, 100)))).receipt;
  browser.close = close;
  assert.equal(receipt.type, "failure");
  assert.equal(receipt.code, "timeout");
  assert.match(receipt.details.cleanup_error, /fixture cleanup failure/u);
  assert.equal(browser.isConnected(), false);
});
