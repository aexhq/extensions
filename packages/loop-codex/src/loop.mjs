import { defineAgentloop } from "@aexhq/agentloop";

export default defineAgentloop({
  step(input) {
    const state = normalizeState(input.context.state);
    const context = { ...input.context, state };
    switch (input.observation.type) {
      case "user_message":
        state.messages.push({ role: "user", content: input.observation.content });
        return model(context, state);
      case "model_completed": {
        const response = input.observation.response?.response ?? input.observation.response ?? {};
        const text = typeof response.text === "string" ? response.text : "";
        const calls = normalizeCalls(response.tool_calls);
        state.messages.push({ role: "assistant", content: text, tool_calls: calls });
        if (calls.length > 0) {
          state.pending = calls;
          return nextTool(context, state);
        }
        state.result = text;
        return { context, decision: { type: "emit", event: { type: "assistant_message", content: text } } };
      }
      case "tools_completed": {
        const result = input.observation.results[0];
        if (result !== undefined) {
          state.messages.push({ role: "tool", tool_call_id: result.call_id, content: result.output, is_error: result.is_error });
        }
        state.pending.shift();
        return state.pending.length > 0 ? nextTool(context, state) : model(context, state);
      }
      case "emitted":
        return { context, decision: { type: "finish", result: state.result } };
      case "cancelled":
        return { context, decision: { type: "fail", code: "cancelled", message: "turn cancelled" } };
      case "session_started":
        return { context, decision: { type: "finish" } };
      default:
        return { context, decision: { type: "fail", code: "unknown_observation", message: "unsupported observation" } };
    }
  },
});

function model(context, state) {
  return { context, decision: { type: "model", request: { messages: state.messages } } };
}

function nextTool(context, state) {
  return { context, decision: { type: "tools", calls: [state.pending[0]] } };
}

function normalizeState(value) {
  if (value && typeof value === "object" && Array.isArray(value.messages) && Array.isArray(value.pending)) return value;
  return { messages: [], pending: [], result: undefined };
}

function normalizeCalls(value) {
  if (!Array.isArray(value)) return [];
  return value.map((call, index) => ({
    callId: String(call.id ?? call.call_id ?? `call_${index}`),
    name: String(call.name ?? call.function?.name ?? ""),
    input: parseArguments(call.input ?? call.function?.arguments),
  })).filter((call) => call.name.length > 0);
}

function parseArguments(value) {
  if (typeof value !== "string") return value ?? {};
  try { return JSON.parse(value); } catch { return { value }; }
}
