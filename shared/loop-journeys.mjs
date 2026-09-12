import assert from "node:assert/strict";
import test from "node:test";
import { hostEnv, tool } from "@aexhq/brain";
import { z } from "zod";
import { answer, calls, collect, fixture } from "./journey-fixture.mjs";

export function loopJourneys(name, loop) {
  test(`${name}: user PDFs and selected Tool PDFs reach the provider as native files`, { timeout: 30_000 }, async t => {
    const file = { type: "file", media_type: "application/pdf", url: "https://example.com/report.pdf" };
    const f = await fixture(t, [
      body => {
        assert.ok(body.input.some(item => item.content?.[0]?.file_url === file.url));
        return calls(["report", {}]);
      },
      body => {
        assert.deepEqual(body.input.at(-1).output, [{ type: "input_text", text: "report ready" }, { type: "input_file", file_url: file.url }]);
        return answer("report understood");
      },
    ]);
    const report = tool({ name: "report", description: "Read report", input: z.object({}), run: () => ({ type: "aex_tool_output", version: 1, content: "report ready", media: [file] }) });
    const session = await f.session(loop, { tools: [report({ env: hostEnv({ name: "app" }) })] });
    await session.send({ message: "Read this report", media: [file] });
    const messages = (await session.transcript()).messages;
    assert.deepEqual(messages[0].content[1], file);
    assert.deepEqual(messages.find(message => message.content[0]?.type === "tool_result").content[0].media, [file]);
  });

  for (const selection of ["hidden", "model"]) {
    test(`${name}: image input and ${selection} placement reach the selected Tool`, { timeout: 30_000 }, async (t) => {
      const invoked = [];
      const f = await fixture(t, [
        (body) => {
          assert.deepEqual(body.input[1].content[0], { type: "input_image", image_url: "https://example.com/view.png" });
          const schema = body.tools[0].parameters;
          if (selection === "model") assert.deepEqual(schema.properties.environment.enum, ["left", "right"]);
          else assert.equal(schema.properties.environment, undefined);
          return calls(["lookup", selection === "model" ? { environment: "right", input: {} } : {}]);
        },
        (body) => { assert.match(body.input.at(-1).output, /right/u); return answer("done"); },
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
        assert.deepEqual(body.input.filter(message => message.type === "function_call_output").map(message => message.output), ["first", "second"]);
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
      () => { const response = calls(["lookup", {}]); response.native = [{ type: "reasoning", encrypted_content: "retain reasoning", summary: [] }]; return response; },
      (body) => {
        assert.match(body.input.at(-1).output, /lookup unavailable/u);
        assert.equal(body.input.find(message => message.type === "reasoning").encrypted_content, "retain reasoning");
        return answer("please supply the missing value");
      },
      (body) => {
        assert.ok(body.input.some(message => message.type === "reasoning" && message.encrypted_content === "retain reasoning"));
        assert.ok(body.input.some(message => message.content === "please supply the missing value"));
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

  test(`${name}: mixed image, text and failed siblings keep source order through the compiled loop`, { timeout: 30_000 }, async t => {
    const image = { type: "image", url: "https://example.com/diagram.png" };
    const releaseImage = Promise.withResolvers();
    const f = await fixture(t, [
      () => calls(["lookup", { kind: "image" }], ["lookup", { kind: "text" }], ["lookup", { kind: "error" }]),
      body => {
        const results = body.input.filter(item => item.type === "function_call_output");
        assert.deepEqual(results.map(item => item.call_id), ["call-0", "call-1", "call-2"]);
        assert.deepEqual(results[0].output, [{ type: "input_text", text: "diagram ready" }, { type: "input_image", image_url: image.url }]);
        assert.equal(results[1].output, "plain text");
        assert.match(results[2].output, /^ERROR: .*sibling unavailable/u);
        return answer("all results understood");
      },
    ]);
    const lookup = tool({ name: "lookup", description: "Lookup", input: z.object({ kind: z.string() }), run: async ({ kind }) => {
      if (kind === "error") throw new Error("sibling unavailable");
      if (kind === "text") return "plain text";
      if (name === "pi") await releaseImage.promise;
      return { type: "aex_tool_output", version: 1, content: "diagram ready", media: [image] };
    } });
    const session = await f.session(loop, { tools: [lookup({ env: hostEnv({ name: "app" }) })] });
    const laterResults = [];
    const observe = name === "pi" ? (async () => {
      for await (const event of session.stream()) {
        if (event.type !== "tool_call_ended") continue;
        laterResults.push(event.data.result.call_id);
        if (laterResults.length === 2) { releaseImage.resolve(); break; }
      }
    })() : Promise.resolve();
    await session.send("lookup all three");
    await observe;
    if (name === "pi") assert.deepEqual(laterResults.sort(), ["call-1", "call-2"]);
    const results = (await session.transcript()).messages.find(message => message.content[0]?.type === "tool_result").content;
    assert.deepEqual(results[0].media, [image]);
    assert.equal(results[2].is_error, true);
  });

  test(`${name}: model failure preserves acknowledged input for explicit continuation`, { timeout: 30_000 }, async (t) => {
    const f = await fixture(t, [
      () => ({ error: "provider unavailable" }),
      (body) => {
        assert.ok(JSON.stringify(body.input).includes("original request"));
        assert.ok(JSON.stringify(body.input).includes("turn_failed"));
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
    const f = await fixture(t, [() => calls(["wait", {}]), body => {
      const callIndex = body.input.findIndex(item => item.type === "function_call");
      const result = body.input[callIndex + 1];
      assert.equal(result.type, "function_call_output");
      assert.equal(result.call_id, body.input[callIndex].call_id);
      assert.match(result.output, /^ERROR: .*Turn interrupted.*operation may have run/u);
      assert.ok(JSON.stringify(body.input).includes("turn_failed"));
      return answer("continued after interruption");
    }]);
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
    await session.interrupt();
    await aborted.promise;
    await running;
    assert.equal(invocations, 1);
    assert.ok((await collect(session.events())).some(event => event.type === "turn_failed"));
    await session.send("continue without repeating the operation");
    assert.equal(invocations, 1);
  });

  for (const stop of ["stop", "length"]) {
    test(`${name}: ${stop === "stop" ? "completed" : "truncated"} compaction through the compiled Component`, { timeout: 30_000 }, async (t) => {
      const configuration = name === "pi"
        ? { compaction: true, contextWindow: 300, reserveTokens: 100, keepRecentTokens: 40 }
        : { compaction: true, contextWindow: 300 };
      const script = [(body) => {
        assert.ok(!body.tools?.length);
        assert.equal(body.text?.format, undefined);
        assert.ok(body.input.some(item => item.content?.[0]?.file_url === "https://example.com/diagram.pdf"));
        return answer("checkpoint: preserve cyan", { stop });
      }];
      if (stop === "stop") script.push(body => {
        assert.match(JSON.stringify(body.input), /checkpoint: preserve cyan/u);
        return answer("continued after compaction");
      });
      const f = await fixture(t, script);
      const session = await f.session(loop, { configuration });
      const original = "original task cyan ".repeat(100);
      await session.send({ message: original, media: [{ type: "file", media_type: "application/pdf", url: "https://example.com/diagram.pdf" }] }).catch(() => {});
      const messages = (await session.transcript()).messages;
      if (stop === "length") {
        assert.equal(messages[0].content[0].text, original);
        assert.ok(!JSON.stringify(messages).includes("checkpoint: preserve cyan"));
        assert.ok((await collect(session.events())).some(event => event.type === "turn_failed"));
      } else assert.equal(messages.at(-1).content[0].text, "continued after compaction");
    });
  }
}
