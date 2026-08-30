import { brain } from "@aexhq/brain";
import { z } from "zod";

const stateSchema = z.object({
  messages: z.array(z.unknown()),
  pending: z.array(z.object({ callId: z.string(), name: z.string(), input: z.unknown() })),
});

export const pi = brain((author) => {
  const state = author.state(stateSchema, () => ({ messages: [], pending: [] }));

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
    return turn.tools(calls);
  });

  author.on.tools((completed, turn) => {
    state.messages.push({
      role: "user",
      content: completed.results.map((result) => ({
        type: "tool_result",
        tool_use_id: result.call_id,
        content: result.output,
        is_error: result.is_error,
      })),
    });
    state.pending = [];
    return turn.model({ messages: state.messages });
  });
});
