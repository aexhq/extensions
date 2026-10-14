export function toolOutput(content, media = []) {
  return { type: "aex_tool_output", version: 1, content, media };
}
