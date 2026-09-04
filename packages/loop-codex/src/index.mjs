import { agentloop } from "@aexhq/brain";
import { z } from "zod";

// Semantic port of the Codex agent loop, pinned against openai/codex tag
// rust-v0.151.0 (= npm @openai/codex 0.151.0): codex-rs/core/src/session/turn.rs
// (run_turn), codex-rs/core/src/compact.rs (local compaction), and the prompts
// under codex-rs/prompts/templates/compact/.
//
// The published packages cannot be imported here: @openai/codex ships only the
// precompiled Rust binary and @openai/codex-sdk spawns it as a subprocess. This
// loop drives one whole turn through Brain's services instead and reproduces
// the loop's contract:
// - each sampling step re-sends the full history; tool calls execute one at
//   a time (codex's default per-tool gate is exclusive) and every output is
//   appended in the original call order before the next sampling step
// - the turn ends when a response carries no tool calls
// - automatic compaction at ~90% of the context window, using codex's local
//   path: summarize via a model call, then replace history with the prior
//   plain user messages (up to ~20k tokens, most recent kept) plus one
//   bridge message carrying the summary
// Codex's remote/server-side compaction, TokenBudget feature, MCP hooks,
// steering queue, and sandbox machinery have no Brain equivalent and are
// not ported.

const optionsSchema = z.object({
  contextWindow: z.number().int().positive().default(200_000),
  compaction: z.boolean().default(true),
}).default({});

// codex-rs/protocol/src/openai_models.rs auto_compact_token_limit(): 90% of
// the resolved context window.
const AUTO_COMPACT_RATIO = 0.9;
// codex-rs/core/src/compact.rs COMPACT_USER_MESSAGE_MAX_TOKENS.
const COMPACT_USER_MESSAGE_MAX_TOKENS = 20_000;

// codex-rs/prompts/templates/compact/prompt.md, verbatim.
const SUMMARIZATION_PROMPT = `You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task.

Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences
- What remains to be done (clear next steps)
- Any critical data, examples, or references needed to continue

Be concise, structured, and focused on helping the next LLM seamlessly continue the work.`;

// codex-rs/prompts/templates/compact/summary_prefix.md, verbatim.
const SUMMARY_PREFIX = `Another language model started to solve this problem and produced a summary of its thinking process. You also have access to the state of the tools that were used by that language model. Use this to build on the work that has already been done and avoid duplicating work. Here is the summary produced by the other language model, use the information in this summary to assist with your own analysis:`;

// Codex counts the tokens the provider reported for the last response; the
// threshold is checked before the first sampling request of a turn, so the
// count carries over between turns.
const usageSchema = z.object({ lastTokens: z.number().nonnegative() });

const messageText = (message) =>
  message.content
    .filter((block) => block.type === "text")
    .map((block) => block.text)
    .join("");

const isPlainUserMessage = (message) =>
  message.role === "user" &&
  message.content.every((block) => block.type === "text") &&
  !messageText(message).startsWith(SUMMARY_PREFIX);

export const codex = agentloop({ options: optionsSchema }, (author) => {
  const options = author.options;
  const usage = author.slot("usage", usageSchema, () => ({ lastTokens: 0 }));

  // The client-side estimate is only the fallback for providers that report
  // nothing.
  const usedTokens = (transcript) =>
    usage.lastTokens > 0 ? usage.lastTokens : author.context.estimateTokens(transcript);

  const shouldCompact = (transcript) =>
    options.compaction && usedTokens(transcript) >= Math.floor(options.contextWindow * AUTO_COMPACT_RATIO);

  // codex-rs/core/src/compact.rs build_compacted_history: prior plain user
  // messages, most recent kept within the token budget (the oldest kept one is
  // truncated to fit in codex; dropped whole here), then one bridge user
  // message carrying the summary, last.
  const compact = async (turn) => {
    const { message } = await turn.model({
      messages: [...turn.transcript, { role: "user", content: [{ type: "text", text: SUMMARIZATION_PROMPT }] }],
    });
    const kept = [];
    let budget = COMPACT_USER_MESSAGE_MAX_TOKENS;
    for (let index = turn.transcript.length - 1; index >= 0; index -= 1) {
      const candidate = turn.transcript[index];
      if (!isPlainUserMessage(candidate)) continue;
      const cost = author.context.estimateTokens([candidate]);
      if (cost > budget) break;
      budget -= cost;
      kept.unshift(candidate);
    }
    kept.push({ role: "user", content: [{ type: "text", text: `${SUMMARY_PREFIX}\n${messageText(message)}` }] });
    turn.transcript.splice(0, turn.transcript.length, ...kept);
    usage.lastTokens = 0;
  };

  author.turn(async (turn) => {
    turn.transcript.push({ role: "user", content: [{ type: "text", text: turn.input.message }] });
    for (;;) {
      // run_pre_sampling_compact before the first request of the turn, and the
      // mid-turn check after each step's tools have completed.
      if (shouldCompact(turn.transcript)) await compact(turn);
      const response = await turn.model({ messages: turn.transcript });
      usage.lastTokens = (response.usage.input_tokens ?? 0) + (response.usage.output_tokens ?? 0);
      turn.transcript.push(response.message);
      const calls = response.message.content
        .filter((block) => block.type === "tool_use")
        .map((block) => ({ callId: block.id, name: block.name, input: block.input }));
      if (calls.length === 0) {
        await turn.reply(messageText(response.message));
        return turn.done();
      }
      const results = [];
      for (const call of calls) {
        const [result] = await turn.dispatch([call]);
        results.push({
          type: "tool_result",
          tool_use_id: call.callId,
          content: result === undefined ? "Tool produced no result." : result.output,
          is_error: result === undefined ? true : result.isError,
        });
      }
      turn.transcript.push({ role: "user", content: results });
    }
  });
});
