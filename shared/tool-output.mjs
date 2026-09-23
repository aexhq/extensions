import { validateMedia } from "./media.mjs";
export function toolOutput(content, media = []) {
  return { type: "aex_tool_output", version: 1, content, media };
}

export function resultBlock(callId, result) {
  const block = { type: "tool_result", tool_use_id: callId,
    content: result === undefined ? "Tool produced no result." : result.content ?? result.output,
    is_error: result === undefined ? true : result.is_error };
  const output = result?.output;
  if (!block.is_error && output?.type === "aex_tool_output" && output.version === 1) {
    if (!Array.isArray(output.media)) {
      throw new TypeError("aex_tool_output media must be an array");
    }
    output.media.forEach(validateMedia);
    block.content = result.content ?? output.content;
    if (output.media.length) block.media = output.media;
  }
  return block;
}

/** One provider reply describes what dispatch actually observed, including an open execution. */
export function toolResult(callId, returned) {
  if (returned === undefined) throw new Error(`dispatch omitted Tool call ${callId}`);
  const results = returned.events.filter(event => event.event_type === "tool_result_emitted")
    .map(event => event.data.result);
  const terminal = returned.events.find(event => event.event_type === "tool_call_ended")?.data.outcome;
  if (terminal !== undefined && terminal.status !== "ok") {
    const failure = { call_id: callId, is_error: true, output: terminal.status === "error"
      ? terminal.error : { code: terminal.status, message: terminal.message ?? `Tool ${terminal.status}` } };
    const previous = results.at(-1);
    if (!(previous?.is_error && previous.content !== undefined && JSON.stringify(previous.output) === JSON.stringify(failure.output))) results.push(failure);
  }
  const blocks = results.map(result => resultBlock(callId, result));
  if (returned.finished && blocks.length === 1) return blocks[0];
  const media = blocks.flatMap(block => block.media ?? []);
  return { type: "tool_result", tool_use_id: callId,
    content: { status: returned.finished ? "finished" : "running",
      results: blocks.map(block => ({ content: block.content, is_error: block.is_error })) },
    is_error: blocks.some(block => block.is_error), ...(media.length ? { media } : {}) };
}
