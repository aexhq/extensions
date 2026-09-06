import assert from "node:assert/strict";
import { once } from "node:events";
import { createServer } from "node:http";
import { Brain, brainEnv, hostEnv, tool } from "@aexhq/brain";
import { pi } from "@aexhq/agentloop-pi";
import { codex } from "@aexhq/agentloop-codex";
import { z } from "zod";

let selection;
let requests = [];
let modelError;
const model = createServer(async (request, response) => {
  try {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = JSON.parse(Buffer.concat(chunks).toString());
    requests.push(body);
    const delta = requests.length === 1 ? { tool_calls: [{ index: 0, id: "lookup-1", type: "function", function: {
      name: "lookup", arguments: JSON.stringify(selection === "model" ? { environment: "right", input: {} } : {}),
    } }] } : { content: "done" };
    response.writeHead(200, { "content-type": "text/event-stream" });
    response.end(`data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: requests.length === 1 ? "tool_calls" : "stop" }] })}\n\ndata: [DONE]\n\n`);
  } catch (error) {
    modelError = error;
    response.writeHead(500).end("invalid test model request");
  }
});
model.listen(Number(process.env.BRAIN_MODEL_PORT ?? 18093), "127.0.0.1");
await once(model, "listening");
const brain = new Brain({ baseUrl: process.env.BRAIN_BASE_URL ?? "http://127.0.0.1:18092", token: "extension-fixture-token", timeoutMs: 120_000 });
try {
  for (const loop of [pi, codex]) {
    for (selection of ["hidden", "model"]) {
      requests = [];
      const called = [];
      const lookup = tool({ name: "lookup", description: "Lookup", input: z.object({}), output: z.object({ where: z.string() }),
        options: z.object({ where: z.string() }),
        run: (_, context) => { called.push(context.options.where); return { where: context.options.where }; },
      });
      const session = await brain.sessions.create({
        agentloop: loop({ env: brainEnv({ name: "brain" }), compaction: false, environmentSelection: selection, placements: { lookup: "right" } }),
        model: { provider: "vercel-ai-gateway", name: "test/extension", apiKey: "test" },
        tools: [lookup({ env: hostEnv({ name: "left" }), where: "left" }), lookup({ env: hostEnv({ name: "right" }), where: "right" })],
      });
      try {
        assert.equal((await session.send("lookup on the right")).status, "idle");
        assert.deepEqual(called, ["right"]);
        assert.equal(requests.length, 2);
        const parameters = requests[0].tools[0].function.parameters;
        if (selection === "model") assert.deepEqual(parameters.properties.environment.enum, ["left", "right"]);
        else assert.equal(parameters.properties.environment, undefined);
        const events = [];
        for await (const event of session.events()) events.push(event);
        const dispatched = events.find((event) => event.type === "tool_call_started");
        assert.equal(dispatched.data.environment, "right");
        assert.deepEqual(dispatched.data.invocation.input, {});
      } finally {
        await session.end();
        await session.delete();
      }
    }
  }
  assert.equal(modelError, undefined);
  console.log("Built Agentloop Components execute both placement modes through brain-env and hostEnv");
} finally {
  model.close();
  await once(model, "close");
}
