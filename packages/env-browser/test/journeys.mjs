import assert from "node:assert/strict";
import test from "node:test";
import { browser, browserTools } from "../dist/index.mjs";
import { serveEnvironment } from "../dist/server.mjs";
import { pi } from "../../loop-pi/dist/index.mjs";
import { codex } from "../../loop-codex/dist/index.mjs";
import { fixture, calls, answer, collect } from "../../../shared/journey-fixture.mjs";
import { browserFixture } from "./fixture.mjs";

for (const [name, loop] of [["pi", pi], ["codex", codex]]) {
  test(`Browser Environment with ${name}: form state and screenshot reach the model through Brain`, { timeout: 60_000 }, async t => {
    let website;
    const f = await fixture(t, [
      () => calls(["browser_navigate", { url: website }]),
      () => calls(["browser_fill", { selector: "#name", value: "Ada" }]),
      () => calls(["browser_click", { selector: "#save" }]),
      () => calls(["browser_screenshot", {}]),
      body => {
        const blocks = body.messages.flatMap(message => Array.isArray(message.content) ? message.content : []);
        assert.ok(blocks.some(block => block.type === "image_url" && block.image_url.url.startsWith("data:image/png;base64,")));
        return answer("form captured");
      },
      () => calls(["browser_inspect", {}]),
      body => { assert.match(body.messages.at(-1).content, /Ada/u); return answer("state retained"); },
    ]);
    const b = await browserFixture(t);
    website = b.url;
    const server = await serveEnvironment(b.env.handle, { token: "browser-fixture" });
    t.after(server.close);
    const env = browser({ name: "browser", url: server.url, token: "browser-fixture", profile: "test" });
    const session = await f.session(loop, { tools: browserTools({ env }) });
    assert.equal((await session.send("fill and screenshot")).status, "idle");
    assert.equal((await session.send("inspect saved state")).status, "idle");
    assert.equal(b.launched.length, 1);
    const events = await collect(session.events());
    assert.equal(events.filter(event => event.type === "tool_call_started").length, 5);
    assert.ok((await session.transcript()).messages.some(message => message.content.some(block => block.type === "tool_result" && block.media?.length)));
  });
}
