import { parseJson, usage } from "@aexhq/model";

export function buildRequest(request) {
  const messages = parseJson(request.messagesJson, "messagesJson");
  const tools = parseJson(request.toolsJson, "toolsJson");
  const generation = parseJson(request.generationJson, "generationJson");
  const options = parseJson(request.providerOptionsJson, "providerOptionsJson");
  if (!Array.isArray(messages) || !Array.isArray(tools) || !isObject(generation) || !isObject(options)) {
    throw new Error("model request JSON fields have invalid root types");
  }
  // The session's sealed instructions arrive on the generation prefix, not in the history, and
  // this dialect carries them as the leading system message.
  const systemPrompt = generation.system_prompt ?? generation.systemPrompt;
  const body = {
    model: request.model,
    stream: true,
    stream_options: { include_usage: true },
    messages: [
      ...(typeof systemPrompt === "string" && systemPrompt !== ""
        ? [{ role: "system", content: systemPrompt }]
        : []),
      ...messages.flatMap(renderMessage),
    ],
  };
  if (tools.length > 0) {
    body.tools = tools.map((tool) => ({
      type: "function",
      function: { name: tool.name, description: tool.description, parameters: tool.input_schema ?? tool.inputSchema },
    }));
  }
  if (generation.tool_choice_none === true || generation.toolChoiceNone === true) body.tool_choice = "none";
  copy(body, "temperature", generation.temperature);
  const outputTokenParameter = options.outputTokenParameter ?? options.output_token_parameter;
  if (outputTokenParameter !== "max_tokens" && outputTokenParameter !== "max_completion_tokens") {
    throw new Error("providerOptions.outputTokenParameter is invalid");
  }
  copy(body, outputTokenParameter, generation.max_tokens ?? generation.maxTokens);
  copy(body, "reasoning_effort", generation.reasoning_effort ?? generation.reasoningEffort);
  const stops = generation.stop_sequences ?? generation.stopSequences;
  if (Array.isArray(stops) && stops.length > 0) body.stop = stops;
  if (request.responseFormatJson !== undefined) body.response_format = parseJson(request.responseFormatJson, "responseFormatJson");
  const baseUrl = requireString(options.base_url ?? options.baseUrl, "providerOptions.baseUrl").replace(/\/+$/u, "");
  const apiKey = requireString(options.api_key ?? options.apiKey, "providerOptions.apiKey");
  return {
    method: "POST",
    url: `${baseUrl}/v1/chat/completions`,
    headers: [["content-type", "application/json"], ["accept", "text/event-stream"], ["authorization", `Bearer ${apiKey}`]],
    body: new TextEncoder().encode(JSON.stringify(body)),
    credential: undefined,
    deadlineAtMs: request.deadlineAtMs,
  };
}

export function decodeFrame(_event, data) {
  if (data.trim() === "[DONE]") return [];
  const value = parseJson(data, "OpenAI SSE frame");
  if (value.error !== undefined) throw new Error(`provider error frame: ${JSON.stringify(value.error)}`);
  const events = [];
  const frameUsage = openAiUsage(value.usage);
  for (const choice of Array.isArray(value.choices) ? value.choices : []) {
    const delta = isObject(choice.delta) ? choice.delta : {};
    if (typeof delta.content === "string" && delta.content !== "") events.push(event("text-delta", { index: 0, text: delta.content }));
    if (typeof delta.refusal === "string" && delta.refusal !== "") events.push(event("refusal-delta", { index: 0, text: delta.refusal }));
    for (const call of Array.isArray(delta.tool_calls) ? delta.tool_calls : []) {
      const index = (Number.isSafeInteger(call.index) ? call.index : 0) + 1;
      const fn = isObject(call.function) ? call.function : {};
      if (typeof call.id === "string" && typeof fn.name === "string") {
        events.push(event("tool-use-start", { index, id: call.id, name: fn.name }));
      }
      if (typeof fn.arguments === "string" && fn.arguments !== "") {
        events.push(event("tool-input-delta", { index, partialJson: fn.arguments }));
      }
    }
    if (typeof choice.finish_reason === "string") {
      events.push(Object.keys(frameUsage).length === 0
        ? { stopReason: mapStop(choice.finish_reason) }
        : { ...event("usage", frameUsage), stopReason: mapStop(choice.finish_reason) });
    }
  }
  if (Object.keys(frameUsage).length > 0 && !events.some((item) => item.kind === "usage")) {
    events.push(event("usage", frameUsage));
  }
  return events;
}

function renderMessage(message) {
  if (!isObject(message) || typeof message.role !== "string") throw new Error("messages must contain objects with roles");
  if (typeof message.content === "string") return [{ role: message.role, content: message.content }];
  if (!Array.isArray(message.content)) throw new Error("message content must be a string or block array");
  if (message.role === "assistant") {
    let text = "";
    const toolCalls = [];
    for (const block of message.content) {
      const type = block.type ?? block.kind;
      if (type === "text") text += requireString(block.text, "text block text");
      else if (type === "tool_use" || type === "tool-use") toolCalls.push({ id: block.id, type: "function", function: { name: block.name, arguments: JSON.stringify(block.input) } });
      else throw new Error(`unsupported assistant block ${String(type)}`);
    }
    return [{ role: "assistant", content: text === "" ? null : text, ...(toolCalls.length === 0 ? {} : { tool_calls: toolCalls }) }];
  }
  if (message.role === "user") {
    let text = "";
    const rendered = [];
    for (const block of message.content) {
      const type = block.type ?? block.kind;
      if (type === "text") text += requireString(block.text, "text block text");
      else if (type === "tool_result" || type === "tool-result") rendered.push({ role: "tool", tool_call_id: block.tool_use_id ?? block.toolUseId, content: block.is_error === true ? `ERROR: ${block.content}` : String(block.content) });
      else throw new Error(`unsupported user block ${String(type)}`);
    }
    if (text !== "") rendered.push({ role: "user", content: text });
    return rendered;
  }
  return [{ role: message.role, content: message.content.map((block) => block.text ?? "").join("") }];
}

function openAiUsage(value) {
  if (!isObject(value)) return {};
  return usage({
    inputTokens: number(value.prompt_tokens), outputTokens: number(value.completion_tokens),
    cacheReadInputTokens: number(value.prompt_tokens_details?.cached_tokens),
    reasoningTokens: number(value.completion_tokens_details?.reasoning_tokens),
  });
}

function mapStop(value) {
  if (value === "stop") return "end_turn";
  if (value === "tool_calls" || value === "function_call") return "tool_use";
  if (value === "length") return "max_tokens";
  if (value === "content_filter") return "refusal";
  return "unknown";
}

function event(kind, payload) { return { kind, payload }; }
function isObject(value) { return value !== null && typeof value === "object" && !Array.isArray(value); }
function number(value) { return Number.isSafeInteger(value) && value >= 0 ? value : undefined; }
function requireString(value, field) { if (typeof value !== "string" || value === "") throw new Error(`${field} must be a non-empty string`); return value; }
// The sealed prefix serializes an unset sampling field as JSON null. Absent stays absent: a
// forwarded null is a value, and providers reject it (`expected number, received null`).
function copy(target, key, value) { if (value !== undefined && value !== null) target[key] = value; }
