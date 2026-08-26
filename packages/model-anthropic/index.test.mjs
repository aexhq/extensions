import assert from "node:assert/strict";
import test from "node:test";
import { prepareComponent } from "@aexhq/brain";
import { anthropic } from "./index.mjs";
import { buildRequest, decodeFrame } from "./provider.mjs";

const request = { model: "claude-test", messagesJson: JSON.stringify([{ role: "user", content: [{ type: "text", text: "hi" }] }]), toolsJson: "[]", responseFormatJson: undefined, generationJson: JSON.stringify({ max_tokens: 100, system_prompt: "system" }), providerOptionsJson: JSON.stringify({ baseUrl: "https://api.example", apiKey: "secret" }), deadlineAtMs: 1n };

test("factory declares a precompiled Model component", async () => {
  const value = anthropic();
  assert.equal(value.extension, "model");
  assert.equal(value.config.provider, "anthropic");
  assert.ok((await prepareComponent(value)).bytes > 0);
  assert.throws(() => anthropic({ baseUrl: "http://example.com" }), /HTTPS/u);
});

test("request and stream codec preserve tools and absent usage", () => {
  const built = buildRequest(request);
  assert.equal(built.url, "https://api.example/v1/messages");
  const body = JSON.parse(new TextDecoder().decode(built.body));
  assert.equal(body.system[0].cache_control.type, "ephemeral");
  const events = decodeFrame("message_start", JSON.stringify({ message: { usage: { input_tokens: 11 } } }));
  assert.deepEqual(events[0].payload, { inputTokens: 11 });
  const done = decodeFrame("message_delta", JSON.stringify({ delta: { stop_reason: "tool_use" }, usage: { output_tokens: 7 } }));
  assert.equal(done[0].stopReason, "tool_use");
  assert.deepEqual(done[0].payload, { outputTokens: 7 });
});

test("an unset sampling field stays absent", () => {
  // The sealed prefix serializes every unset field as null; forwarding one is a 400.
  const body = JSON.parse(new TextDecoder().decode(buildRequest({
    ...request,
    generationJson: JSON.stringify({ max_tokens: 100, system_prompt: "system", temperature: null, stop_sequences: null }),
  }).body));
  assert.equal("temperature" in body, false);
  assert.equal("stop_sequences" in body, false);
});

test("the sealed prefix always carries reasoning effort, and only a chosen one is refused", () => {
  const unset = JSON.parse(new TextDecoder().decode(buildRequest({
    ...request,
    generationJson: JSON.stringify({ max_tokens: 100, system_prompt: "system", reasoning_effort: null }),
  }).body));
  assert.equal(unset.model, "claude-test");

  assert.throws(() => buildRequest({
    ...request,
    generationJson: JSON.stringify({ max_tokens: 100, system_prompt: "system", reasoning_effort: "high" }),
  }), /not supported/u);
});
