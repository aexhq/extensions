import { agentloop } from "@aexhq/brain";
import { z } from "zod";

// Semantic port of the Codex agent loop, pinned against openai/codex tag
// rust-v0.151.0 (= npm @openai/codex 0.151.0): codex-rs/core/src/session/turn.rs
// (run_turn), codex-rs/core/src/compact.rs (local compaction), and the prompts
// under codex-rs/prompts/templates/compact/.
//
// The published packages cannot be imported here: @openai/codex ships only the
// precompiled Rust binary and @openai/codex-sdk spawns it as a subprocess, so
// nothing runs inside Brain's deterministic synchronous sandbox. This port
// reproduces the loop's contract instead:
// - each step re-sends the full history; tool outputs are appended in the
//   original call order before the next sampling step, and calls execute one
//   at a time (codex's default per-tool gate is exclusive)
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

const stateSchema = z.object({
  messages: z.array(z.unknown()),
  pending: z.array(z.object({ callId: z.string(), name: z.string(), input: z.unknown() })),
  results: z.array(z.unknown()),
  compacting: z.boolean(),
  lastTokens: z.number().nonnegative(),
});

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
  const state = author.state(stateSchema, () => ({
    messages: [],
    pending: [],
    results: [],
    compacting: false,
    lastTokens: 0,
  }));

  // Codex counts the tokens the provider reported for the last response, not a
  // client-side estimate; the estimate is only the fallback for providers that
  // report nothing.
  const usedTokens = () =>
    state.lastTokens > 0 ? state.lastTokens : author.context.estimateTokens(state.messages);

  const shouldCompact = () =>
    options.compaction && usedTokens() >= Math.floor(options.contextWindow * AUTO_COMPACT_RATIO);

  const compact = (turn) => {
    state.compacting = true;
    return turn.model({
      messages: [
        ...state.messages,
        { role: "user", content: [{ type: "text", text: SUMMARIZATION_PROMPT }] },
      ],
    });
  };

  // codex-rs/core/src/compact.rs build_compacted_history: prior plain user
  // messages, most recent kept within the token budget (the oldest kept one is
  // truncated to fit in codex; dropped whole here), then one bridge user
  // message carrying the summary, last.
  const rebuild = (summary) => {
    const kept = [];
    let budget = COMPACT_USER_MESSAGE_MAX_TOKENS;
    for (let index = state.messages.length - 1; index >= 0; index -= 1) {
      const message = state.messages[index];
      if (!isPlainUserMessage(message)) continue;
      const cost = author.context.estimateTokens([message]);
      if (cost > budget) break;
      budget -= cost;
      kept.unshift(message);
    }
    kept.push({ role: "user", content: [{ type: "text", text: `${SUMMARY_PREFIX}\n${summary}` }] });
    state.messages = kept;
  };

  const advance = (turn) => (shouldCompact() ? compact(turn) : turn.model({ messages: state.messages }));

  author.on.message((message, turn) => {
    const text = typeof message.content === "string" ? message.content : JSON.stringify(message.content);
    state.messages.push({ role: "user", content: [{ type: "text", text }] });
    // run_pre_sampling_compact: the threshold is checked before the first
    // sampling request of every turn.
    return advance(turn);
  });

  author.on.model((completed, turn) => {
    const { message, usage } = completed.response;
    if (state.compacting) {
      state.compacting = false;
      rebuild(messageText(message));
      state.lastTokens = 0;
      return turn.model({ messages: state.messages });
    }
    state.lastTokens = (usage.input_tokens ?? 0) + (usage.output_tokens ?? 0);
    const calls = message.content
      .filter((block) => block.type === "tool_use")
      .map((block) => ({ callId: block.id, name: block.name, input: block.input }));
    state.messages.push(message);
    if (calls.length === 0) return turn.reply(messageText(message));
    state.pending = calls;
    state.results = [];
    return turn.tools([calls[0]]);
  });

  author.on.tools((completed, turn) => {
    const call = state.pending.shift();
    const result = completed.results[0];
    state.results.push({
      type: "tool_result",
      tool_use_id: call?.callId ?? result?.call_id ?? "unknown",
      content: result === undefined ? "Tool produced no result." : result.output,
      is_error: result === undefined ? true : result.is_error,
    });
    if (state.pending.length > 0) return turn.tools([state.pending[0]]);
    // Outputs land in the original call order before the next sampling step.
    state.messages.push({ role: "user", content: state.results });
    state.results = [];
    // The mid-turn threshold check: after a step whose tools have completed,
    // before the next sampling request.
    return advance(turn);
  });
});
