export function officialTool(name: string): Readonly<{ type: "aex_official_tool"; version: 1; name: string }> {
  return Object.freeze({ type: "aex_official_tool", version: 1, name });
}
