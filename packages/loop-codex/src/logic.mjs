import { observeEvents } from "../../../shared/loop-events.mjs";

const AUTO_COMPACT_RATIO = 0.9;
const COMPACT_USER_MESSAGE_MAX_TOKENS = 20_000;

const SUMMARIZATION_PROMPT = `You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task.

Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences
- What remains to be done (clear next steps)
- Any critical data, examples, or references needed to continue

Be concise, structured, and focused on helping the next LLM seamlessly continue the work.`;

const SUMMARY_PREFIX = `Another language model started to solve this problem and produced a summary of its thinking process. You also have access to the state of the tools that were used by that language model. Use this to build on the work that has already been done and avoid duplicating work. Here is the summary produced by the other language model, use the information in this summary to assist with your own analysis:`;

const estimateTokens = (messages) => Math.ceil(JSON.stringify(messages).length / 4);
const messageText = (message) => message.content.filter((block) => block.type === "text").map((block) => block.text).join("");
const isPlainUserMessage = (message) =>
  message.role === "user" && message.content.every((block) => block.type === "text") && !messageText(message).startsWith(SUMMARY_PREFIX);

export async function runCodex(input, context) {
  const options = { contextWindow: 200_000, compaction: true, ...input.configuration };
  const transcript = cloneJson(input.transcript);
  const observed_sequence = await observeEvents(context, transcript, input.slots.observed_sequence ?? 0);
  const saved = input.slots.usage;
  const usage = saved === undefined ? { lastTokens: 0 } : cloneJson(saved);
  const usedTokens = () => usage.lastTokens > 0 ? usage.lastTokens : estimateTokens(transcript);
  const shouldCompact = () => options.compaction && usedTokens() >= Math.floor(options.contextWindow * AUTO_COMPACT_RATIO);
  const compact = async () => {
    const { message } = await context.model({
      messages: [...transcript, { role: "user", content: [{ type: "text", text: SUMMARIZATION_PROMPT }] }],
    });
    const kept = [];
    let budget = COMPACT_USER_MESSAGE_MAX_TOKENS;
    for (let index = transcript.length - 1; index >= 0; index -= 1) {
      const candidate = transcript[index];
      if (!isPlainUserMessage(candidate)) continue;
      const cost = estimateTokens([candidate]);
      if (cost > budget) break;
      budget -= cost;
      kept.unshift(candidate);
    }
    kept.push({ role: "user", content: [{ type: "text", text: `${SUMMARY_PREFIX}\n${messageText(message)}` }] });
    transcript.splice(0, transcript.length, ...kept);
    usage.lastTokens = 0;
  };

  transcript.push({ role: "user", content: [{ type: "text", text: input.input.message }] });
  for (;;) {
    if (shouldCompact()) await compact();
    const response = await context.model({ messages: transcript });
    usage.lastTokens = (response.usage.input_tokens ?? 0) + (response.usage.output_tokens ?? 0);
    transcript.push(response.message);
    const calls = response.message.content
      .filter((block) => block.type === "tool_use")
      .map((block) => ({ call_id: block.id, name: block.name, input: block.input }));
    if (calls.length === 0) {
      await context.emit("output_emitted", { type: "assistant_message", message: messageText(response.message) });
      return { transcript, slots: { usage, observed_sequence } };
    }
    const results = [];
    for (const call of calls) {
      const [result] = await context.dispatch([call]);
      results.push({
        type: "tool_result",
        tool_use_id: call.call_id,
        content: result === undefined ? "Tool produced no result." : result.output,
        is_error: result === undefined ? true : result.is_error,
      });
    }
    transcript.push({ role: "user", content: results });
  }
}

const cloneJson = (value) => JSON.parse(JSON.stringify(value));
