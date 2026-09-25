import assert from "node:assert/strict";
import test from "node:test";
import { hostEnv } from "@aexhq/brain";
import { env } from "../packages/env/dist/index.js";
import { browser, browserTools } from "../packages/env-browser/dist/index.mjs";
import { serveEnvironment } from "../packages/env-browser/dist/server.mjs";
import { browserFixture } from "../packages/env-browser/test/fixture.mjs";
import { pi } from "../packages/loop-pi/dist/index.mjs";
import { codex } from "../packages/loop-codex/dist/index.mjs";
import { fixture, calls, answer, collect } from "../shared/journey-fixture.mjs";
import { eventually } from "../shared/environment-test-fixture.mjs";

for (const [name, loop] of [["pi", pi], ["codex", codex]]) {
  test(`env with ${name}: manual setup, dynamic placement, resource controls and idle observations`, { timeout: 120_000 }, async t => {
    let session;
    const reference = async name => (await session.environments.list()).find(view => view.reference.name === name).reference;
    const manage = input => calls(["env", { environment: "app", input }]);
    const f = await fixture(t, [
      () => calls(["browser_inspect", { environment: "browser", input: {} }]),
      body => { assert.match(JSON.stringify(body.input), /environment_not_ready/u); return manage({ operation: "list" }); },
      () => manage({ operation: "setup", environment: { name: "browser", sequence: 1 } }),
      () => manage({ operation: "create", template: "browser", name: "second", configuration: { profile: "test" } }),
      async () => manage({ operation: "setup", environment: await reference("second") }),
      async () => manage({ operation: "call", environment: await reference("second"), method: "open_page", input: { name: "report" } }),
      body => {
        assert.ok(body.tools.find(tool => tool.name === "browser_inspect").parameters.properties.environment.enum.includes("second"));
        return calls(["browser_inspect", { environment: "second", input: {} }]);
      },
      () => manage({ operation: "delete", environment: { name: "browser", sequence: 1 } }),
      body => {
        assert.deepEqual(body.tools.find(tool => tool.name === "browser_inspect").parameters.properties.environment.enum, ["second"]);
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
    session = await f.session(loop, {
      environmentLifecycle: { default: "automatic", bindings: { browser: "manual" } },
      configuration: { environmentSelection: "model", environments: [{ environment: "browser", permissions: ["read"], methods: [] }] },
      tools: [...browserTools({ env: workspace }), env({ env: hostEnv({ name: "app" }), environments: [grant] })],
    });
    assert.equal((await session.send("prepare and inspect the browser")).status, "idle");
    const events = await collect(session.events());
    assert.deepEqual(events.filter(event => event.type === "turn_failed"), []);
    const dispatched = events.find(event => event.type === "tool_call_started" && event.data.environment === "second");
    assert.ok(dispatched, "the loop dispatches into the dynamic binding");
    await b.launched[0].close();
    await eventually(async () => JSON.stringify(await session.transcript()).includes("browser loss observed"));
  });
}
