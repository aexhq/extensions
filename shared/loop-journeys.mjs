import assert from "node:assert/strict";
import test from "node:test";
import { hostEnv, tool } from "@aexhq/brain";
import { z } from "zod";
import { answer, calls, collect, fixture } from "./journey-fixture.mjs";

export function loopJourneys(name, loop) {
  for (const selection of ["hidden", "model"]) {
    test(`${name}: image input and ${selection} placement reach the selected Tool`, { timeout: 30_000 }, async (t) => {
      const invoked = [];
      const f = await fixture(t, [
        (body) => {
          assert.deepEqual(body.messages[0].content[1], { type: "image_url", image_url: { url: "https://example.com/view.png" } });
          const schema = body.tools[0].function.parameters;
          if (selection === "model") assert.deepEqual(schema.properties.environment.enum, ["left", "right"]);
          else assert.equal(schema.properties.environment, undefined);
          return calls(["lookup", selection === "model" ? { environment: "right", input: {} } : {}]);
        },
        (body) => { assert.match(body.messages.at(-1).content, /right/u); return answer("done"); },
      ]);
      const lookup = tool({ name: "lookup", description: "Lookup", input: z.object({}),
        options: z.object({ where: z.string() }),
        run: (_, context) => { invoked.push(context.options.where); return { where: context.options.where }; },
      });
      const session = await f.session(loop, {
        configuration: { environmentSelection: selection, placements: { lookup: "right" } },
        tools: [lookup({ env: hostEnv({ name: "left" }), where: "left" }), lookup({ env: hostEnv({ name: "right" }), where: "right" })],
      });
      assert.equal((await session.send({ message: "lookup", media: [{ type: "image", url: "https://example.com/view.png" }] })).status, "idle");
      assert.deepEqual(invoked, ["right"]);
      const events = await collect(session.events());
      assert.equal(events.find(event => event.type === "tool_call_started").data.environment, "right");
      const output = events.find(event => event.type === "output_emitted");
      assert.equal(output.data.message, "done");
      assert.equal(output.origin.kind, "agentloop");
    });
  }

  test(`${name}: compiled Tool batches preserve ${name === "pi" ? "parallel" : "sequential"} execution and result order`, { timeout: 30_000 }, async (t) => {
    const bothEntered = Promise.withResolvers();
    let entered = 0;
    const f = await fixture(t, [
      () => calls(["lookup", { value: "first" }], ["lookup", { value: "second" }]),
      body => {
        assert.deepEqual(body.messages.filter(message => message.role === "tool").map(message => message.content), ["first", "second"]);
        return answer("batch complete");
      },
    ]);
    const lookup = tool({ name: "lookup", description: "Lookup", input: z.object({ value: z.string() }), run: async ({ value }) => {
      if (++entered === 2) bothEntered.resolve();
      if (name === "pi") await bothEntered.promise;
      return value;
    } });
    const session = await f.session(loop, { tools: [lookup({ env: hostEnv({ name: "app" }) })] });
    await session.send("lookup both values");
    assert.equal(entered, 2);
    const events = await collect(session.events());
    const starts = events.filter(event => event.type === "tool_call_started");
    const ends = events.filter(event => event.type === "tool_call_ended");
    assert.equal(starts.length, 2);
    assert.equal(ends.length, 2);
    if (name === "pi") assert.ok(starts[1].sequence < ends[0].sequence);
    else assert.ok(ends[0].sequence < starts[1].sequence);
  });

  test(`${name}: Tool errors reach the model once and native history survives the next turn`, { timeout: 30_000 }, async (t) => {
    let invocations = 0;
    const f = await fixture(t, [
      () => { const response = calls(["lookup", {}]); response.delta.reasoning_content = "retain reasoning"; return response; },
      (body) => {
        assert.match(body.messages.at(-1).content, /lookup unavailable/u);
        assert.equal(body.messages.find(message => message.role === "assistant").reasoning_content, "retain reasoning");
        return answer("please supply the missing value");
      },
      (body) => {
        assert.ok(body.messages.some(message => message.reasoning_content === "retain reasoning"));
        assert.ok(body.messages.some(message => message.content === "please supply the missing value"));
        return answer("finished with supplied value");
      },
    ]);
    const lookup = tool({ name: "lookup", description: "Lookup", input: z.object({}),
      run: () => { invocations++; throw new Error("lookup unavailable"); },
    });
    const session = await f.session(loop, { tools: [lookup({ env: hostEnv({ name: "app" }) })] });
    await session.send("lookup a value");
    await session.send("the value is cyan");
    assert.equal(invocations, 1);
    const messages = (await session.transcript()).messages;
    assert.ok(messages.some(message => message.content.some(block => block.type === "native")));
    assert.equal(messages.at(-1).content[0].text, "finished with supplied value");
  });

  test(`${name}: model failure preserves acknowledged input for explicit continuation`, { timeout: 30_000 }, async (t) => {
    const f = await fixture(t, [
      () => ({ error: "provider unavailable" }),
      (body) => {
        assert.ok(JSON.stringify(body.messages).includes("original request"));
        assert.ok(JSON.stringify(body.messages).includes("turn_failed"));
        return answer("continued");
      },
    ]);
    const session = await f.session(loop);
    await session.send("original request").catch(() => {});
    assert.equal((await session.transcript()).messages[0].content[0].text, "original request");
    assert.ok((await collect(session.events())).some(event => event.type === "turn_failed"));
    await session.send("continue");
    assert.equal(f.requests.length, 2);
  });

  test(`${name}: cancellation stops a waiting Tool without replay`, { timeout: 30_000 }, async (t) => {
    const entered = Promise.withResolvers();
    const aborted = Promise.withResolvers();
    let invocations = 0;
    const f = await fixture(t, [() => calls(["wait", {}])]);
    const wait = tool({ name: "wait", description: "Wait", input: z.object({}), run: async (_, context) => {
      invocations++;
      context.signal.addEventListener("abort", () => aborted.resolve(), { once: true });
      entered.resolve();
      await aborted.promise;
      return "cancelled";
    } });
    const session = await f.session(loop, { tools: [wait({ env: hostEnv({ name: "app" }) })] });
    const running = session.send("wait").catch(() => {});
    await entered.promise;
    await session.cancel();
    await aborted.promise;
    await running;
    assert.equal(invocations, 1);
    assert.ok((await collect(session.events())).some(event => event.type === "turn_failed"));
  });

  for (const stop of ["stop", "length"]) {
    test(`${name}: ${stop === "stop" ? "completed" : "truncated"} compaction through the compiled Component`, { timeout: 30_000 }, async (t) => {
      const configuration = name === "pi"
        ? { compaction: true, contextWindow: 300, reserveTokens: 100, keepRecentTokens: 40 }
        : { compaction: true, contextWindow: 300 };
      const script = [(body) => {
        assert.ok(!body.tools?.length);
        assert.equal(body.response_format, undefined);
        return answer("checkpoint: preserve cyan", { stop });
      }];
      if (stop === "stop") script.push(body => {
        assert.match(JSON.stringify(body.messages), /checkpoint: preserve cyan/u);
        return answer("continued after compaction");
      });
      const f = await fixture(t, script);
      const session = await f.session(loop, { configuration });
      const original = "original task cyan ".repeat(100);
      await session.send(original).catch(() => {});
      const messages = (await session.transcript()).messages;
      if (stop === "length") {
        assert.equal(messages[0].content[0].text, original);
        assert.ok(!JSON.stringify(messages).includes("checkpoint: preserve cyan"));
        assert.ok((await collect(session.events())).some(event => event.type === "turn_failed"));
      } else assert.equal(messages.at(-1).content[0].text, "continued after compaction");
    });
  }
}
