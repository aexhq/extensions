import { toolPlacement, refreshTools } from "../../../shared/tool-placement.mjs";
import { observeEvents } from "../../../shared/loop-events.mjs";
import { toolResult } from "../../../shared/tool-output.mjs";
import { structuredOutput } from "../../../shared/structured-output.mjs";

const AUTO_COMPACT_RATIO = 0.9;
const COMPACT_USER_MESSAGE_MAX_TOKENS = 20_000;

const SUMMARIZATION_PROMPT = `You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task.

Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences
- What remains to be done (clear next steps)
- Any critical data, examples, or references needed to continue

Be concise, structured, and focused on helping the next LLM seamlessly continue the work.`;

const SUMMARY_PREFIX = `Another language model started to solve this problem and produced a summary of its thinking process. You also have access to the kv of the tools that were used by that language model. Use this to build on the work that has already been done and avoid duplicating work. Here is the summary produced by the other language model, use the information in this summary to assist with your own analysis:`;

const estimateTokens = (messages) => Math.ceil(JSON.stringify(messages).length / 4);
const messageText = (message) => message.content.filter((block) => block.type === "text").map((block) => block.text).join("");
const isPlainUserMessage = (message) =>
  message.role === "user" && message.content.every((block) => block.type === "text") && !messageText(message).startsWith(SUMMARY_PREFIX);

export async function runCodex(ctx) {
  const options = { contextWindow: 200_000, compaction: true, ...ctx.configuration };
  const output = structuredOutput(options.output);
  const transcript = cloneJson(ctx.transcript);
  let observed = await observeEvents(ctx, transcript, await ctx.kv.get("observed") ?? 0);
  const saved = await ctx.kv.get("usage");
  const usage = saved === undefined ? { lastTokens: 0 } : cloneJson(saved);
  const usedTokens = () => usage.lastTokens > 0 ? usage.lastTokens : estimateTokens(transcript);
  const shouldCompact = () => options.compaction && usedTokens() >= Math.floor(options.contextWindow * AUTO_COMPACT_RATIO);
  const compact = async () => {
    const { message, stop_reason } = await ctx.model({
      response_format: null,
      tools: [],
      messages: [...transcript, { role: "user", content: [{ type: "text", text: SUMMARIZATION_PROMPT }] }],
    });
    if (stop_reason !== "end_turn") throw new Error(`Compaction did not complete: ${stop_reason}`);
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

  if (ctx.input != null) transcript.push({ role: "user", content: [{ type: "text", text: ctx.input.message }, ...(ctx.input.media ?? [])] });
  await ctx.setTranscript(transcript);
  await ctx.kv.set("observed", observed.through);
  if (ctx.input == null && !observed.actionable) return {};
  for (;;) {
    let placement = toolPlacement(await refreshTools(ctx.tools, ctx), options);
    if (shouldCompact()) {
      await compact();
      await ctx.setTranscript(transcript);
      await ctx.kv.set("usage", usage);
    }
    const response = await ctx.model(output?.request(transcript, placement.definitions) ?? { messages: transcript, tools: placement.definitions });
    usage.lastTokens = (response.usage.total_input_tokens ?? response.usage.input_tokens ?? 0) + (response.usage.output_tokens ?? 0);
    transcript.push(response.message);
    await ctx.setTranscript(transcript);
    await ctx.kv.set("usage", usage);
    const calls = response.message.content
      .filter((block) => block.type === "tool_use")
      .map((block) => ({ call_id: block.id, name: block.name, input: block.input }));
    if (calls.length === 0) {
      const final = output?.accept(messageText(response.message), response.stop_reason);
      if (final?.correction) {
        transcript.push(final.correction);
        await ctx.setTranscript(transcript);
        continue;
      }
      await ctx.emit("output_emitted", { type: "assistant_message", message: messageText(response.message) });
      return final ? { result: final.value } : {};
    }
    if (output?.correcting) output.rejectTools();
    const results = [];
    const consumed = new Set();
    for (const call of calls) {
      placement = toolPlacement(await refreshTools(ctx.tools, ctx), options);
      const [result] = await ctx.callTools([placement.invocation(call)]);
      results.push(toolResult(call.call_id, result));
      for (const event of result.events) consumed.add(event.sequence);
    }
    transcript.push({ role: "user", content: results });
    observed = await observeEvents(ctx, transcript, observed.through, consumed);
    await ctx.setTranscript(transcript);
    await ctx.kv.set("observed", observed.through);
  }
}

const cloneJson = (value) => JSON.parse(JSON.stringify(value));
