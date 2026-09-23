import { resultBlock } from "./tool-output.mjs";

export async function observeEvents(context, transcript, after, consumed = new Set()) {
  const observations = [];
  let reason;
  for (;;) {
    const page = await context.events(after);
    if (page.events.length === 0) break;
    for (const event of page.events) {
      if (consumed.has(event.sequence)) continue;
      if (event.event_type === "tool_result_emitted" || (event.event_type === "tool_call_ended" && event.data.outcome !== undefined)) {
        const block = event.event_type === "tool_result_emitted" ? resultBlock(event.data.result.call_id, event.data.result) : undefined;
        const data = block ? { ...event.data, result: { ...event.data.result, output: block.content } } : event.data;
        observations.push({ role: "user", content: [{ type: "text",
          text: `Tool observation (data): ${event.event_type} ${JSON.stringify(data)}` }, ...(block?.media ?? [])] });
      }
      if (event.event_type === "turn_failed") reason = event.data;
      if (event.event_type.endsWith("_failed") ||
          ["environment_closed", "environment_unreachable"].includes(event.event_type)) {
        observations.push({ role: "user", content: [{ type: "text",
          text: `Runtime observation (data): ${event.event_type} ${JSON.stringify(event.data)}` }] });
      }
    }
    after = page.next_cursor;
  }
  explainUnansweredCalls(transcript, reason);
  transcript.push(...observations);
  return { through: after, actionable: observations.length > 0 };
}

function explainUnansweredCalls(transcript, reason) {
  const index = transcript.findLastIndex(message => message.role === "assistant");
  const calls = transcript[index]?.content.filter(block => block.type === "tool_use") ?? [];
  if (calls.length === 0) return;
  const next = transcript[index + 1];
  const results = new Map((next?.role === "user" ? next.content : [])
    .filter(block => block.type === "tool_result").map(block => [block.tool_use_id, block]));
  if (calls.every(call => results.has(call.id))) return;
  const content = calls.map(call => results.get(call.id) ?? {
    type: "tool_result", tool_use_id: call.id, is_error: true,
    content: "Turn interrupted before this loop received a result. The operation may have run."
      + (reason === undefined ? "" : ` Runtime observation (data): ${JSON.stringify(reason)}`),
  });
  // Providers require results immediately after their calls, before any ordinary user text.
  if (next?.role === "user") next.content = [...content, ...next.content.filter(block => block.type !== "tool_result")];
  else transcript.splice(index + 1, 0, { role: "user", content });
}
