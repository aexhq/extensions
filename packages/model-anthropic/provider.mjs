import { parseJson, usage } from "@aexhq/model";

export function buildRequest(request) {
  const messages = parseJson(request.messagesJson, "messagesJson");
  const tools = parseJson(request.toolsJson, "toolsJson");
  const generation = parseJson(request.generationJson, "generationJson");
  const options = parseJson(request.providerOptionsJson, "providerOptionsJson");
  if (!Array.isArray(messages) || !Array.isArray(tools) || !isObject(generation) || !isObject(options)) {
    throw new Error("model request JSON fields have invalid root types");
  }
  // The sealed prefix always carries this field, null when unset, so only a chosen effort is one.
  if ((generation.reasoning_effort ?? generation.reasoningEffort) != null) {
    throw new Error("reasoning_effort is not supported by the Anthropic MVP profile");
  }
  const body = {
    model: request.model,
    max_tokens: generation.max_tokens ?? generation.maxTokens,
    stream: true,
    system: generation.system_prompt ?? generation.systemPrompt ?? "",
    messages: messages.filter((message) => message.role !== "system").map(renderMessage),
  };
  const system = messages.find((message) => message.role === "system");
  if (system !== undefined) body.system = renderSystem(system);
  if (tools.length > 0) {
    body.tools = tools.map((tool) => ({ name: tool.name, description: tool.description, input_schema: tool.input_schema ?? tool.inputSchema }));
    body.tools.at(-1).cache_control = { type: "ephemeral" };
    if (generation.tool_choice_none === true || generation.toolChoiceNone === true) body.tool_choice = { type: "none" };
  } else {
    body.system = [{ type: "text", text: typeof body.system === "string" ? body.system : body.system.map((block) => block.text ?? "").join(""), cache_control: { type: "ephemeral" } }];
  }
  copy(body, "temperature", generation.temperature);
  const stops = generation.stop_sequences ?? generation.stopSequences;
  if (Array.isArray(stops) && stops.length > 0) body.stop_sequences = stops;
  const baseUrl = requireString(options.base_url ?? options.baseUrl, "providerOptions.baseUrl").replace(/\/+$/u, "");
  const apiKey = requireString(options.api_key ?? options.apiKey, "providerOptions.apiKey");
  return {
    method: "POST",
    url: `${baseUrl}/v1/messages`,
    headers: [["content-type", "application/json"], ["accept", "text/event-stream"], ["anthropic-version", "2023-06-01"], ["x-api-key", apiKey]],
    body: new TextEncoder().encode(JSON.stringify(body)),
    credential: undefined,
    deadlineAtMs: request.deadlineAtMs,
  };
}

export function decodeFrame(eventName, data) {
  if (data === "[DONE]") return [];
  const value = parseJson(data, "Anthropic SSE frame");
  const type = eventName ?? value.type ?? "";
  const index = Number.isSafeInteger(value.index) ? value.index : 0;
  if (type === "content_block_start" && value.content_block?.type === "tool_use") {
    return [event("tool-use-start", { index, id: value.content_block.id ?? "", name: value.content_block.name ?? "" })];
  }
  if (type === "content_block_delta" && value.delta?.type === "text_delta") {
    return [event("text-delta", { index, text: value.delta.text ?? "" })];
  }
  if (type === "content_block_delta" && value.delta?.type === "input_json_delta") {
    return [event("tool-input-delta", { index, partialJson: value.delta.partial_json ?? "" })];
  }
  if (type === "message_start" && value.message?.usage !== undefined) {
    return [event("usage", anthropicUsage(value.message.usage))];
  }
  if (type === "message_delta") {
    const frameUsage = anthropicUsage(value.usage);
    return [Object.keys(frameUsage).length === 0
      ? { stopReason: mapStop(value.delta?.stop_reason) }
      : { ...event("usage", frameUsage), stopReason: mapStop(value.delta?.stop_reason) }];
  }
  if (type === "error") throw new Error(`provider error frame: ${JSON.stringify(value.error ?? value)}`);
  return [];
}

function renderMessage(message) {
  if (!isObject(message) || (message.role !== "user" && message.role !== "assistant")) throw new Error("Anthropic messages require user or assistant roles");
  const blocks = typeof message.content === "string" ? [{ type: "text", text: message.content }] : message.content;
  if (!Array.isArray(blocks)) throw new Error("message content must be a string or block array");
  return { role: message.role, content: blocks.map((block) => {
    const type = block.type ?? block.kind;
    if (type === "text") return { type: "text", text: requireString(block.text, "text block text") };
    if (type === "tool_use" || type === "tool-use") return { type: "tool_use", id: block.id, name: block.name, input: block.input };
    if (type === "tool_result" || type === "tool-result") return { type: "tool_result", tool_use_id: block.tool_use_id ?? block.toolUseId, content: String(block.content), is_error: block.is_error === true || block.isError === true };
    throw new Error(`unsupported message block ${String(type)}`);
  }) };
}

function renderSystem(message) {
  if (typeof message.content === "string") return message.content;
  if (!Array.isArray(message.content)) throw new Error("system content must be a string or block array");
  return message.content.map((block) => ({ type: "text", text: requireString(block.text, "system text") }));
}

function anthropicUsage(value) {
  if (!isObject(value)) return {};
  return usage({
    inputTokens: number(value.input_tokens), outputTokens: number(value.output_tokens),
    cacheReadInputTokens: number(value.cache_read_input_tokens), cacheCreationInputTokens: number(value.cache_creation_input_tokens),
  });
}

function mapStop(value) {
  if (value === "end_turn") return "end_turn";
  if (value === "tool_use") return "tool_use";
  if (value === "max_tokens") return "max_tokens";
  if (value === "stop_sequence") return "stop_sequence";
  if (value === "refusal") return "refusal";
  return "unknown";
}

function event(kind, payload) { return { kind, payload }; }
function isObject(value) { return value !== null && typeof value === "object" && !Array.isArray(value); }
function number(value) { return Number.isSafeInteger(value) && value >= 0 ? value : undefined; }
function requireString(value, field) { if (typeof value !== "string" || value === "") throw new Error(`${field} must be a non-empty string`); return value; }
// The sealed prefix serializes an unset sampling field as JSON null. Absent stays absent: a
// forwarded null is a value, and providers reject it (`expected number, received null`).
function copy(target, key, value) { if (value !== undefined && value !== null) target[key] = value; }
