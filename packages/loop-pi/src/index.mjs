import { agentloop } from "@aexhq/brain";
import { z } from "zod";

// Semantic port of the pi coding agent's loop, pinned against
// earendil-works/pi tag v0.84.4 (@earendil-works/pi-agent-core@0.84.4,
// packages/agent/src/agent-loop.ts + packages/coding-agent/src/core/compaction/compaction.ts).
//
// The loop drives one whole turn through Brain's services: it calls the model,
// hands tool calls to Brain, and edits the transcript Brain persists. What it
// reproduces of pi:
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

// The checkpoint is the transcript's first message once a compaction has
// happened; the slot remembers its text so the next compaction can update it.
const checkpointSchema = z.object({ summary: z.union([z.string(), z.null()]) });

const CHECKPOINT_PREFIX = "Context checkpoint from earlier in this conversation:\n\n";

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

const text = (message) =>
  message.content
    .filter((block) => block.type === "text")
    .map((block) => block.text)
    .join("");

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
  const checkpoint = author.slot("checkpoint", checkpointSchema, () => ({ summary: null }));

  // The conversation proper: the transcript minus the checkpoint message.
  const body = (transcript) => (checkpoint.summary === null ? transcript : transcript.slice(1));

  const shouldCompact = (transcript) =>
    options.compaction && author.context.estimateTokens(transcript) > options.contextWindow - options.reserveTokens;

  // Earliest index the recent-token budget allows, then forward to the next
  // boundary that does not split a tool_result away from its tool_use.
  const cutPoint = (messages) => {
    let kept = 0;
    let cut = 0;
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      kept += author.context.estimateTokens([messages[index]]);
      if (kept > options.keepRecentTokens) {
        cut = index + 1;
        break;
      }
    }
    while (
      cut < messages.length &&
      messages[cut].role === "user" &&
      messages[cut].content.some((block) => block.type === "tool_result")
    ) {
      cut += 1;
    }
    return cut;
  };

  const compact = async (turn) => {
    const messages = body(turn.transcript);
    const cut = cutPoint(messages);
    if (cut === 0) return;
    const previous = checkpoint.summary === null ? "" : `Previous summary:\n\n${checkpoint.summary}\n\n`;
    const prompt = checkpoint.summary === null ? SUMMARIZATION_PROMPT : `${UPDATE_RULES}${SUMMARIZATION_PROMPT}`;
    const { message } = await turn.model({
      messages: [
        {
          role: "user",
          content: [{ type: "text", text: `${previous}${serializeConversation(messages.slice(0, cut))}\n\n${prompt}` }],
        },
      ],
    });
    checkpoint.summary = text(message);
    turn.transcript.splice(
      0,
      turn.transcript.length,
      { role: "user", content: [{ type: "text", text: `${CHECKPOINT_PREFIX}${checkpoint.summary}` }] },
      ...messages.slice(cut),
    );
  };

  author.turn(async (turn) => {
    turn.transcript.push({ role: "user", content: [{ type: "text", text: turn.input.message }] });
    for (;;) {
      if (shouldCompact(turn.transcript)) await compact(turn);
      const { message, stop_reason } = await turn.model({ messages: turn.transcript });
      turn.transcript.push(message);
      const calls = message.content
        .filter((block) => block.type === "tool_use")
        .map((block) => ({ callId: block.id, name: block.name, input: block.input }));
      if (calls.length === 0) {
        await turn.reply(text(message));
        return turn.done();
      }
      if (stop_reason === "max_tokens") {
        // pi fails the whole batch without executing it when the response was
        // cut off: the arguments cannot be trusted.
        turn.transcript.push({
          role: "user",
          content: calls.map((call) => ({
            type: "tool_result",
            tool_use_id: call.callId,
            content: TRUNCATED_CALL_MESSAGE,
            is_error: true,
          })),
        });
        continue;
      }
      // One dispatch: Brain runs the batch in parallel and reports back once.
      const results = await turn.dispatch(calls);
      const byCall = new Map(results.map((result) => [result.callId, result]));
      turn.transcript.push({
        role: "user",
        // Assistant source order, the order pi appends result messages in.
        content: calls.map(({ callId }) => {
          const result = byCall.get(callId);
          return {
            type: "tool_result",
            tool_use_id: callId,
            content: result === undefined ? "Tool produced no result." : result.output,
            is_error: result === undefined ? true : result.isError,
          };
        }),
      });
    }
  });
});
