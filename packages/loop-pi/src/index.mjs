import { agentloop } from "@aexhq/brain";
import { z } from "zod";

// Semantic port of the pi coding agent's loop, pinned against
// earendil-works/pi tag v0.84.4 (@earendil-works/pi-agent-core@0.84.4,
// packages/agent/src/agent-loop.ts + packages/coding-agent/src/core/compaction/compaction.ts).
//
// The published loop is an async driver: one invocation awaits the LLM stream
// and every tool execution internally, so it cannot run inside Brain's
// deterministic synchronous sandbox. This port reproduces its per-turn
// contract instead:
// - tool calls are issued as one parallel batch, results return as one
//   user message of tool_result blocks in assistant source order
// - a length-stop that carries tool calls fails the whole batch without
//   executing it, and the loop re-asks the model
// - automatic compaction: when the estimated context exceeds
//   contextWindow - reserveTokens, everything older than ~keepRecentTokens
//   is summarized into a structured context checkpoint that replaces it
//   (cut points never split a tool_result away from its tool_use)
// Host-app seams of pi (steering and follow-up queues, per-tool
// executionMode) have no Brain equivalent and are not ported.

const optionsSchema = z.object({
  contextWindow: z.number().int().positive().default(200_000),
  // pi defaults (compaction.ts): compact when context exceeds
  // contextWindow - reserveTokens, keep ~keepRecentTokens of recent messages.
  reserveTokens: z.number().int().positive().default(16_384),
  keepRecentTokens: z.number().int().positive().default(20_000),
  compaction: z.boolean().default(true),
}).default({});

const stateSchema = z.object({
  messages: z.array(z.unknown()),
  summary: z.union([z.string(), z.null()]),
  compacting: z.boolean(),
  keepFrom: z.number().int().nonnegative(),
  pending: z.array(z.object({ callId: z.string(), name: z.string() })),
});

const SUMMARIZATION_PROMPT = `The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal

## Constraints & Preferences

## Progress

### Done

### In Progress

### Blocked

## Key Decisions

## Next Steps

## Critical Context

Keep each section concise. Preserve exact file paths, function names, and error messages.`;

const UPDATE_RULES = `Update the existing structured summary with new information.

RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- Move items from In Progress to Done as they complete

`;

const TRUNCATED_CALL_MESSAGE =
  "Tool call was not executed: the model response was cut off by the output token limit, so the arguments may be truncated. Re-issue the tool call.";

const blockText = (block) => {
  if (block.type === "text") return block.text;
  if (block.type === "tool_use") return `[tool_use ${block.name}] ${JSON.stringify(block.input)}`;
  const output = typeof block.content === "string" ? block.content : JSON.stringify(block.content);
  return `[tool_result${block.is_error ? " (error)" : ""}] ${output.length > 2000 ? `${output.slice(0, 2000)}…` : output}`;
};

const serializeConversation = (messages) =>
  messages.map((message) => `${message.role}:\n${message.content.map(blockText).join("\n")}`).join("\n\n");

export const pi = agentloop({ options: optionsSchema }, (author) => {
  const options = author.options;
  const state = author.state(stateSchema, () => ({
    messages: [],
    summary: null,
    compacting: false,
    keepFrom: 0,
    pending: [],
  }));

  const context = () =>
    state.summary === null
      ? state.messages
      : [
          {
            role: "user",
            content: [{ type: "text", text: `Context checkpoint from earlier in this conversation:\n\n${state.summary}` }],
          },
          ...state.messages,
        ];

  const shouldCompact = () =>
    options.compaction && author.context.estimateTokens(context()) > options.contextWindow - options.reserveTokens;

  // Earliest index the recent-token budget allows, then forward to the next
  // boundary that does not split a tool_result away from its tool_use.
  const cutPoint = () => {
    let kept = 0;
    let cut = 0;
    for (let index = state.messages.length - 1; index >= 0; index -= 1) {
      kept += author.context.estimateTokens([state.messages[index]]);
      if (kept > options.keepRecentTokens) {
        cut = index + 1;
        break;
      }
    }
    while (
      cut < state.messages.length &&
      state.messages[cut].role === "user" &&
      state.messages[cut].content.some((block) => block.type === "tool_result")
    ) {
      cut += 1;
    }
    return cut;
  };

  const compact = (turn) => {
    state.keepFrom = cutPoint();
    if (state.keepFrom === 0) return turn.model({ messages: context() });
    state.compacting = true;
    const previous = state.summary === null ? "" : `Previous summary:\n\n${state.summary}\n\n`;
    const prompt = state.summary === null ? SUMMARIZATION_PROMPT : `${UPDATE_RULES}${SUMMARIZATION_PROMPT}`;
    return turn.model({
      messages: [
        {
          role: "user",
          content: [
            {
              type: "text",
              text: `${previous}${serializeConversation(state.messages.slice(0, state.keepFrom))}\n\n${prompt}`,
            },
          ],
        },
      ],
    });
  };

  const advance = (turn) => (shouldCompact() ? compact(turn) : turn.model({ messages: context() }));

  author.on.message(({ input }, turn) => {
    state.messages.push({ role: "user", content: [{ type: "text", text: input.message }] });
    return advance(turn);
  });

  author.on.model((completed, turn) => {
    const { message, stop_reason } = completed.response;
    if (state.compacting) {
      state.summary = message.content
        .filter((block) => block.type === "text")
        .map((block) => block.text)
        .join("");
      state.messages = state.messages.slice(state.keepFrom);
      state.compacting = false;
      state.keepFrom = 0;
      return turn.model({ messages: context() });
    }
    const calls = message.content
      .filter((block) => block.type === "tool_use")
      .map((block) => ({ callId: block.id, name: block.name, input: block.input }));
    state.messages.push(message);
    if (calls.length > 0 && stop_reason === "max_tokens") {
      // pi fails the whole batch without executing it when the response was
      // cut off: the arguments cannot be trusted.
      state.messages.push({
        role: "user",
        content: calls.map((call) => ({
          type: "tool_result",
          tool_use_id: call.callId,
          content: TRUNCATED_CALL_MESSAGE,
          is_error: true,
        })),
      });
      return advance(turn);
    }
    if (calls.length > 0) {
      state.pending = calls.map(({ callId, name }) => ({ callId, name }));
      return turn.tools(calls);
    }
    const text = message.content
      .filter((block) => block.type === "text")
      .map((block) => block.text)
      .join("");
    return turn.reply(text);
  });

  author.on.tools((completed, turn) => {
    const byCall = new Map(completed.results.map((result) => [result.call_id, result]));
    state.messages.push({
      role: "user",
      // Assistant source order, the order pi appends result messages in.
      content: state.pending.map(({ callId }) => {
        const result = byCall.get(callId);
        return {
          type: "tool_result",
          tool_use_id: callId,
          content: result === undefined ? "Tool produced no result." : result.output,
          is_error: result === undefined ? true : result.is_error,
        };
      }),
    });
    state.pending = [];
    return advance(turn);
  });
});
