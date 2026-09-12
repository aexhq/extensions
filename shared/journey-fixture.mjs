import assert from "node:assert/strict";
import { once } from "node:events";
import { createServer } from "node:http";
import { Brain, brainEnv } from "@aexhq/brain";

export const collect = async (events) => Array.fromAsync(events);
export const answer = (text, extra = {}) => ({ text, ...extra });
export const calls = (...values) => ({ output: values.map(([name, input], index) => ({
  type: "function_call", call_id: `call-${index}`, name, arguments: JSON.stringify(input),
})) });

export async function fixture(t, script) {
  const requests = [];
  const errors = [];
  const sessions = [];
  const server = createServer(async (request, response) => {
    try {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      const body = JSON.parse(Buffer.concat(chunks).toString());
      requests.push(body);
      assert.ok(script.length, "unexpected extra model request");
      const next = await script.shift()(body);
      if (next.error) {
        response.writeHead(503).end(JSON.stringify({ error: { message: next.error } }));
        return;
      }
      response.writeHead(200, { "content-type": "text/event-stream" });
      const output = [...(next.native ?? []), ...(next.output ?? [])];
      const frames = output.map((item, output_index) => ({ type: "response.output_item.done", output_index, item }));
      if (next.text !== undefined) {
        frames.push({ type: "response.output_text.delta", output_index: output.length, delta: next.text });
        frames.push({ type: "response.output_item.done", output_index: output.length, item: { type: "message", content: [{ type: "output_text", text: next.text }] } });
      }
      frames.push({ type: next.stop === "length" ? "response.incomplete" : "response.completed", response: {
        output, usage: next.usage, ...(next.stop === "length" ? { incomplete_details: { reason: "max_output_tokens" } } : {}),
      } });
      response.end(frames.map(frame => `data: ${JSON.stringify(frame)}\n\n`).join(""));
    } catch (error) {
      errors.push(error);
      response.writeHead(500).end(String(error));
    }
  });
  server.listen(Number(process.env.BRAIN_MODEL_PORT ?? 18093), "127.0.0.1");
  await once(server, "listening");
  t.after(async () => {
    try {
      for (const session of sessions) { await session.end(); await session.delete(); }
    } finally {
      await new Promise((resolve) => server.close(resolve));
    }
  });
  const brain = new Brain({ baseUrl: process.env.BRAIN_BASE_URL ?? "http://127.0.0.1:18092", token: "extension-fixture-token", timeoutMs: 30_000 });
  return {
    brain, requests,
    async session(loop, { configuration = {}, ...options } = {}) {
      const session = await brain.sessions.create({
        agentloop: loop({ env: brainEnv({ name: "brain" }), compaction: false, ...configuration }),
        model: { provider: "vercel-ai-gateway", name: "test/extension", apiKey: "test" },
        ...options,
      });
      sessions.push(session);
      t.after(() => {
        assert.deepEqual(errors, []);
        assert.equal(script.length, 0, "all scripted model rounds must execute");
      });
      return session;
    },
  };
}
