import { appendFile } from "node:fs/promises";
import { McpServer, ProtocolError } from "@modelcontextprotocol/server";
import { serveStdio } from "@modelcontextprotocol/server/stdio";
import { schemaFixtures } from "./schema-fixtures.mjs";

const journal = process.argv[2];
const record = data => appendFile(journal, `${JSON.stringify(data)}\n`);
const empty = { type: "object", properties: {}, additionalProperties: false };
const definitions = [
  { name: "lookup", description: "Look up a fixture value", inputSchema: { type: "object", properties: { value: { type: "string", enum: ["Ada", "Lin"] } }, required: ["value"], additionalProperties: false },
    outputSchema: { type: "object", properties: { value: { type: "string" } }, required: ["value"] } },
  ...["failure", "protocol_failure", "image", "wait", "disconnect", "change", "continuation"].map(name => ({ name, description: name, inputSchema: empty })),
  ...schemaFixtures.map(({ name, schema }) => ({ name, description: name, inputSchema: schema })),
  { name: "unresolved", description: "Unresolved external reference", inputSchema: { type: "object", properties: { value: { $ref: "https://example.invalid/missing.json" } } } },
];
let changed = false;
const mcp = new McpServer({ name: "aex-mcp-fixture", version: "1" }, {
  supportedProtocolVersions: ["2026-07-28"], capabilities: { tools: { listChanged: true } },
});
const server = mcp.server;
server.setRequestHandler("tools/list", async request => {
  await record({ type: "list", cursor: request.params?.cursor });
  const all = changed ? [...definitions, { name: "added", description: "Added later", inputSchema: empty }] : definitions;
  return request.params?.cursor === "second" ? { tools: all.slice(2) } : { tools: all.slice(0, 2), nextCursor: "second" };
});
server.setRequestHandler("tools/call", async (request, context) => {
  const { name, arguments: input } = request.params;
  await record({ type: "call", name, input });
  if (name === "lookup") return { content: [{ type: "text", text: input.value }], structuredContent: { value: input.value } };
  if (name === "failure") return { isError: true, content: [{ type: "text", text: "permission denied" }], structuredContent: { code: "permission_denied", resource: "fixture" } };
  if (name === "protocol_failure") throw new ProtocolError(-32001, "permission denied", { resource: "fixture" });
  if (schemaFixtures.some(fixture => fixture.name === name)) return { content: [{ type: "text", text: JSON.stringify(input) }], structuredContent: input };
  if (name === "image") return { content: [{ type: "image", mimeType: "image/png", data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aD1sAAAAASUVORK5CYII=" }] };
  if (name === "wait") {
    await new Promise(resolve => context.mcpReq.signal.addEventListener("abort", resolve, { once: true }));
    await record({ type: "cancelled", name });
    return { content: [{ type: "text", text: "cancelled" }] };
  }
  if (name === "disconnect") { await record({ type: "mutation" }); process.exit(0); }
  if (name === "change") { changed = true; return { content: [{ type: "text", text: "catalogue changed" }] }; }
  if (name === "continuation") return { resultType: "input_required", inputRequests: { approval: {
    method: "elicitation/create", params: { mode: "form", message: "Need input", requestedSchema: { type: "object", properties: { answer: { type: "string" } } } },
  } }, requestState: "fixture-state" };
  throw new Error("unknown fixture Tool");
});
serveStdio(() => mcp, { legacy: "reject" });
