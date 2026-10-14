import assert from "node:assert/strict";
import test from "node:test";
import { tool } from "@aexhq/brain";
import { z } from "zod";
import { pi } from "../packages/loop-pi/src/index.mjs";
import { codex } from "../packages/loop-codex/src/index.mjs";
import { opencode } from "../packages/loop-opencode/src/index.mjs";
import { answer, calls, collect, fixture } from "../shared/journey-fixture.mjs";

test("Pi, Codex and OpenCode serve concurrent sessions with isolated tools and configuration", { timeout: 120_000 }, async t => {
  const selections = [
    pi({ contextWindow: 64_000, maxTokens: 1024, compaction: { enabled: false } }),
    codex({ contextWindow: 64_000 }),
    opencode({ contextWindow: 64_000, maxTokens: 1024, compaction: { auto: false } }),
  ];
  const count = 12;
  const executed = [];
  const waiting = Promise.withResolvers();
  const entered = new Set();
  const round = async body => {
    const result = body.input.findLast(item => item.type === "function_call_output");
    if (result) return answer(result.output);
    const label = JSON.stringify(body.input).match(/session-(pi|codex|opencode)-\d+/u)?.[0];
    assert.ok(label, "each upstream preserves its session input");
    entered.add(label.split("-")[1]);
    if (entered.size === 3) waiting.resolve();
    await waiting.promise;
    if (!label.startsWith("session-codex-")) assert.equal(body.max_output_tokens, 1024, `${label}: session settings override startup defaults`);
    return calls([body.tools.find(item => item.name.includes("echo")).name, { label }]);
  };
  const f = await fixture(t, Array.from({ length: count * 3 * 2 }, () => round), { timeoutMs: 110_000 });
  const sessions = await Promise.all(selections.flatMap(agentloop => Array.from({ length: count }, async (_, index) => {
    const label = `session-${agentloop.name}-${index}`;
    const echo = tool({ name: "echo", description: "Echo this session's label", input: z.object({ label: z.string() }),
      run: (input, ctx) => {
        assert.equal(input.label, label, "tool calls cannot cross session boundaries");
        executed.push(label);
        return ctx.finish({ label });
      },
    });
    return { label, session: await f.session({ agentloop, tools: [echo()] }) };
  })));
  await Promise.all(sessions.map(async ({ label, session }) => {
    await session.send(label);
    const events = await collect(session.events());
    assert.ok(events.some(event => event.type === "turn_ended"), JSON.stringify(events.at(-1)));
    assert.ok(!events.some(event => event.type === "turn_failed"), JSON.stringify(events));
    assert.equal(events.filter(event => event.type === "model_call_ended").length, 2);
    assert.equal(events.filter(event => event.type === "tool_call_ended").length, 1);
    const transcript = JSON.stringify(await session.transcript());
    assert.ok(transcript.includes(label));
    assert.ok(!sessions.some(other => other.label !== label && transcript.includes(`"${other.label}"`)));
  }));
  assert.equal(new Set(executed).size, sessions.length);
  assert.equal(executed.length, sessions.length, "each requested effect executes once");
  assert.equal(entered.size, 3, "all implementations make progress together");
});
