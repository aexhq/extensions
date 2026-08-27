import { brain } from "@aexhq/brain";
import { z } from "zod";

const stateSchema = z.object({
  messages: z.array(z.unknown()),
  pending: z.array(z.object({ callId: z.string(), name: z.string(), input: z.unknown() })),
});

export const pi = brain((author) => {
  const state = author.state(stateSchema, () => ({ messages: [], pending: [] }));

  author.on.message((message, turn) => {
    state.messages.push({ role: "user", content: message.content });
    return turn.model({ messages: state.messages });
  });

  author.on.model((completed, turn) => {
    const response = completed.response?.response ?? completed.response ?? {};
    const text = typeof response.text === "string" ? response.text : "";
    const calls = normalizeCalls(response.tool_calls);
    state.messages.push({ role: "assistant", content: text, tool_calls: calls });
    if (calls.length === 0) return turn.reply(text);
    state.pending = calls;
    return turn.tools(calls);
  });

  author.on.tools((completed, turn) => {
    for (const result of completed.results) state.messages.push({ role: "tool", tool_call_id: result.call_id, content: result.output, is_error: result.is_error });
    state.pending = [];
    return turn.model({ messages: state.messages });
  });
});

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
