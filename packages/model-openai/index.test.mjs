import assert from "node:assert/strict";
import test from "node:test";
import { prepareComponent } from "@aexhq/brain";
import { openai } from "./index.mjs";
import { buildRequest, decodeFrame } from "./provider.mjs";

const request = { model: "gpt-test", messagesJson: JSON.stringify([{ role: "user", content: [{ type: "text", text: "hi" }] }]), toolsJson: "[]", responseFormatJson: undefined, generationJson: JSON.stringify({ max_tokens: 100 }), providerOptionsJson: JSON.stringify({ baseUrl: "https://api.example", apiKey: "secret", outputTokenParameter: "max_completion_tokens" }), deadlineAtMs: 1n };

test("factory declares a precompiled Model component", async () => {
  const value = openai();
  assert.equal(value.extension, "model");
  assert.equal(value.config.provider, "openai");
  assert.ok((await prepareComponent(value)).bytes > 0);
  assert.throws(() => openai({ baseUrl: "http://example.com" }), /HTTPS/u);
});

test("request and stream codec preserve tool, usage, and stop semantics", () => {
  const built = buildRequest(request);
  assert.equal(built.url, "https://api.example/v1/chat/completions");
  const body = JSON.parse(new TextDecoder().decode(built.body));
  assert.equal(body.stream_options.include_usage, true);
  assert.equal(body.max_completion_tokens, 100);
  assert.equal(body.max_tokens, undefined);
  const events = decodeFrame(undefined, JSON.stringify({ choices: [{ delta: { tool_calls: [{ index: 0, id: "call-1", function: { name: "read", arguments: "{\"path\":" } }] }, finish_reason: "tool_calls" }], usage: { prompt_tokens: 7 } }));
  assert.deepEqual(events.map((event) => event.kind), ["tool-use-start", "tool-input-delta", "usage"]);
  assert.equal(events.at(-1).payload.inputTokens, 7);
  assert.equal(events.at(-1).payload.outputTokens, undefined);
  assert.equal(events.at(-1).stopReason, "tool_use");
});
