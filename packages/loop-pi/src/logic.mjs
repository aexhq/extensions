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

const estimateTokens = (messages) => Math.ceil(JSON.stringify(messages).length / 4);
const text = (message) => message.content.filter((block) => block.type === "text").map((block) => block.text).join("");

const blockText = (block) => {
  if (block.type === "text") return block.text;
  if (block.type === "tool_use") return `[tool_use ${block.name}] ${JSON.stringify(block.input)}`;
  const output = typeof block.content === "string" ? block.content : JSON.stringify(block.content);
  return `[tool_result${block.is_error ? " (error)" : ""}] ${output.length > 2000 ? `${output.slice(0, 2000)}…` : output}`;
};

const serializeConversation = (messages) =>
  messages.map((message) => `${message.role}:\n${message.content.map(blockText).join("\n")}`).join("\n\n");

export async function runPi(input, context) {
  const options = {
    contextWindow: 200_000,
    reserveTokens: 16_384,
    keepRecentTokens: 20_000,
    compaction: true,
    ...input.configuration,
  };
  const transcript = cloneJson(input.transcript);
  const saved = input.slots.checkpoint;
  const checkpoint = saved === undefined ? { summary: null } : cloneJson(saved);
  const body = () => checkpoint.summary === null ? transcript : transcript.slice(1);
  const shouldCompact = () =>
    options.compaction && estimateTokens(transcript) > options.contextWindow - options.reserveTokens;
  const cutPoint = (messages) => {
    let kept = 0;
    let cut = 0;
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      kept += estimateTokens([messages[index]]);
      if (kept > options.keepRecentTokens) {
        cut = index + 1;
        break;
      }
    }
    while (cut < messages.length && messages[cut].role === "user" && messages[cut].content.some((block) => block.type === "tool_result")) cut += 1;
    return cut;
  };
  const compact = async () => {
    const messages = body();
    const cut = cutPoint(messages);
    if (cut === 0) return;
    const previous = checkpoint.summary === null ? "" : `Previous summary:\n\n${checkpoint.summary}\n\n`;
    const prompt = checkpoint.summary === null ? SUMMARIZATION_PROMPT : `${UPDATE_RULES}${SUMMARIZATION_PROMPT}`;
    const { message } = await context.model({
      messages: [{ role: "user", content: [{ type: "text", text: `${previous}${serializeConversation(messages.slice(0, cut))}\n\n${prompt}` }] }],
    });
    checkpoint.summary = text(message);
    transcript.splice(0, transcript.length,
      { role: "user", content: [{ type: "text", text: `${CHECKPOINT_PREFIX}${checkpoint.summary}` }] },
      ...messages.slice(cut));
  };

  transcript.push({ role: "user", content: [{ type: "text", text: input.input.message }] });
  for (;;) {
    if (shouldCompact()) await compact();
    const { message, stop_reason } = await context.model({ messages: transcript });
    transcript.push(message);
    const calls = message.content
      .filter((block) => block.type === "tool_use")
      .map((block) => ({ call_id: block.id, name: block.name, input: block.input }));
    if (calls.length === 0) {
      await context.emit("output_emitted", { type: "assistant_message", message: text(message) });
      return { transcript, slots: { checkpoint } };
    }
    if (stop_reason === "max_tokens") {
      transcript.push({
        role: "user",
        content: calls.map((call) => ({ type: "tool_result", tool_use_id: call.call_id, content: TRUNCATED_CALL_MESSAGE, is_error: true })),
      });
      continue;
    }
    const results = await context.dispatch(calls);
    const byCall = new Map(results.map((result) => [result.call_id, result]));
    transcript.push({
      role: "user",
      content: calls.map(({ call_id }) => {
        const result = byCall.get(call_id);
        return {
          type: "tool_result",
          tool_use_id: call_id,
          content: result === undefined ? "Tool produced no result." : result.output,
          is_error: result === undefined ? true : result.is_error,
        };
      }),
    });
  }
}

const cloneJson = (value) => JSON.parse(JSON.stringify(value));
