import { validateMedia } from "./media.mjs";
export function toolOutput(content, media = []) {
  return { type: "aex_tool_output", version: 1, content, media };
}

export function toolResult(callId, result) {
  const block = { type: "tool_result", tool_use_id: callId,
    content: result === undefined ? "Tool produced no result." : result.output,
    is_error: result === undefined ? true : result.is_error };
  const output = result?.output;
  if (!block.is_error && output?.type === "aex_tool_output" && output.version === 1) {
    if (!Array.isArray(output.media)) {
      throw new TypeError("aex_tool_output media must be an array");
    }
    output.media.forEach(validateMedia);
    block.content = output.content;
    if (output.media.length) block.media = output.media;
  }
  return block;
}
