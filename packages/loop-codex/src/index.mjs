import { brain } from "@aexhq/brain";
import { z } from "zod";

const stateSchema = z.object({
  messages: z.array(z.unknown()),
  pending: z.array(z.object({ callId: z.string(), name: z.string(), input: z.unknown() })),
  results: z.array(z.unknown()),
});

export const codex = brain((author) => {
  const state = author.state(stateSchema, () => ({ messages: [], pending: [], results: [] }));

  author.on.message((message, turn) => {
    const text = typeof message.content === "string" ? message.content : JSON.stringify(message.content);
    state.messages.push({ role: "user", content: [{ type: "text", text }] });
    return turn.model({ messages: state.messages });
  });

  author.on.model((completed, turn) => {
    const { message } = completed.response;
    const text = message.content
      .filter((block) => block.type === "text")
      .map((block) => block.text)
      .join("");
    const calls = message.content
      .filter((block) => block.type === "tool_use")
      .map((block) => ({ callId: block.id, name: block.name, input: block.input }));
    state.messages.push(message);
    if (calls.length === 0) return turn.reply(text);
    state.pending = calls;
    state.results = [];
    return turn.tools([calls[0]]);
  });

  author.on.tools((completed, turn) => {
    const result = completed.results[0];
    if (result !== undefined) state.results.push({
      type: "tool_result",
      tool_use_id: result.call_id,
      content: result.output,
      is_error: result.is_error,
    });
    state.pending.shift();
    if (state.pending.length > 0) return turn.tools([state.pending[0]]);
    state.messages.push({ role: "user", content: state.results });
    state.results = [];
    return turn.model({ messages: state.messages });
  });
});
