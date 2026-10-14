import assert from "node:assert/strict";
import test from "node:test";
import { hostEnv } from "@aexhq/brain";
import { env } from "../packages/env/dist/index.js";
import { browser, browserTools } from "../packages/env-browser/dist/index.mjs";
import { serveEnvironment } from "../packages/env-browser/dist/server.mjs";
import { browserFixture } from "../packages/env-browser/test/fixture.mjs";
import { fixture, calls, answer, collect } from "../shared/journey-fixture.mjs";
import { eventually } from "../shared/environment-test-fixture.mjs";

test("env with upstream Pi: manual setup, dynamic resources, fixed catalogue and idle observations", { timeout: 120_000 }, async t => {
  let session;
  const reference = async name => (await session.environments.list()).find(view => view.reference.name === name).reference;
  const manage = input => calls(["env", input]);
  const f = await fixture(t, [
    () => calls(["browser_inspect", {}]),
    body => { assert.match(JSON.stringify(body.input), /environment_not_ready/u); return manage({ operation: "list" }); },
    () => manage({ operation: "setup", environment: { name: "browser", sequence: 1 } }),
    () => calls(["browser_inspect", {}]),
    () => manage({ operation: "create", template: "browser", name: "second", configuration: { profile: "test" } }),
    async () => manage({ operation: "setup", environment: await reference("second") }),
    async () => manage({ operation: "call", environment: await reference("second"), method: "open_page", input: { name: "report" } }),
    async body => {
      assert.equal(body.tools.filter(tool => tool.name === "browser_inspect").length, 1);
      return manage({ operation: "call", environment: await reference("second"), method: "inspect", input: {} });
    },
    async body => {
      assert.match(body.input.at(-1).output, /report/u);
      return manage({ operation: "delete", environment: await reference("second") });
    },
    body => {
      assert.equal(body.tools.filter(tool => tool.name === "browser_inspect").length, 1);
      return answer("environment ready");
    },
    body => { assert.match(JSON.stringify(body.input), /environment_observation.*browser disconnected/iu); return answer("browser loss observed"); },
  ], { timeoutMs: 60_000 });
  const b = await browserFixture(t);
  const server = await serveEnvironment(b.env.handle, { token: "browser-fixture" });
  t.after(server.close);
  const workspace = browser({ name: "browser", url: server.url, token: "browser-fixture", profile: "test",
    template: { max_instances: 2, configuration_schema: { type: "object", properties: { profile: { const: "test" } }, required: ["profile"], additionalProperties: false } } });
  const grant = { environment: "browser", permissions: ["read", "create", "setup", "delete", "call"], methods: ["inspect", "open_page"] };
  session = await f.session({
    environment: { lifecycle: { bindings: { browser: "manual" } } },
    tools: [...browserTools({ env: workspace }), env({ env: hostEnv({ name: "app" }), environments: [grant] })],
  });
  assert.equal((await session.send("prepare and inspect the browser")).status, "idle");
  const events = await collect(session.events());
  assert.deepEqual(events.filter(event => event.type === "turn_failed"), []);
  assert.equal((await session.environments.list()).find(view => view.reference.name === "second").state, "deleted");
  assert.ok(events.some(event => event.type === "tool_call_started" && event.data.tool === "env"));
  await b.launched[0].close();
  await eventually(async () => JSON.stringify(await session.transcript()).includes("browser loss observed"));
});
