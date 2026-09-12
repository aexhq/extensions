# @aexhq/tools-mcp

Connect selected MCP Tools to Brain through application `hostEnv`. The application owns the MCP connection, authentication and lifecycle. The bridge never accepts arbitrary model-selected Tool names.

```js
import { hostEnv } from "@aexhq/brain";
import { StdioClientTransport } from "@modelcontextprotocol/client/stdio";
import { connectMcp, mcpTools } from "@aexhq/tools-mcp";

const client = await connectMcp(new StdioClientTransport({
  command: "node", args: ["./my-mcp-server.mjs"],
}), { versionNegotiation: { mode: { pin: "2026-07-28" } } });

const tools = await mcpTools({
  client, env: hostEnv({ name: "app" }), names: ["search", "lookup"], prefix: "service_",
});
// Supply tools at session creation. Keep the client alive while these bindings are used.
// After ending their sessions:
// await client.close();
```

The client SDK owns transport and protocol negotiation. `connectMcp` disables automatic interactive fulfilment; specify the protocol revision your server supports. A connected client can also be supplied directly. The bridge uses a fixed discovered Tool definition on every call, so an SDK cache refresh cannot change the session's schemas or trigger the header-mismatch resend path. Discovery uses the SDK's bounded pagination. Later list changes do not add or replace placed Tools; authorize a new session for changed definitions.

## Results and failures

MCP images and embedded image/PDF resources require a caller-supplied
`publishMedia({ bytes, mediaType, index }, context)` callback on `mcpTools`. Return an HTTPS URL
reachable by the model provider. `context` supplies the session ID, invocation sequence, deadline
and cancellation signal. The application owns upload authorization and expiry, and can bind this
callback to Aex's attachment SDK or another store.

Known binary media is published before the normalized `mcp_result` Event is committed, including
MCP error results. The retained evidence contains URL media and the remaining MCP content;
image/base64 and resource blobs are removed. A missing or failed publisher produces
`mcp_media_failed` evidence and a failed Tool result. The completed MCP tool is never repeated to
repair publication. Audio, other binary resource types, asynchronous tasks and interactive
continuations fail explicitly.

Successful text, structured content and resource links are presented to the model. Published PNG,
JPEG, GIF, WebP and PDF inputs use `aex_tool_output` version 1. Use the matching URL-media Pi/Codex
release to map them into Brain Tool-result media. Unsupported binary fields inside arbitrary Tool
JSON are not recursively transformed.

An optional `project(output, toolName)` selects a smaller model-visible JSON representation. Its input includes `content`, optional `structuredContent`, and the committed `evidenceSequence`; normalized evidence remains in the Event. Image and PDF media is carried separately. Brain's existing emitted-Event and model-input limits still apply; exceeding them is an explicit failure.

MCP `isError` becomes a failed Brain Tool result with code `mcp_tool_error`, `retryable: false` and the normalized MCP result in `details`. JSON-RPC failures use `mcp_protocol_error` and retain the original numeric code and data in `details`. Known MCP SDK failures use `mcp_sdk_error`; unsupported continuations use `mcp_unsupported_result`. Protocol/transport errors also produce `mcp_failure` evidence while the invocation is active.

Invocation deadlines and MCP request timeouts produce `timeout`; explicit SDK cancellation produces `cancelled`. The bridge returns `unknown` only when an invocation may have been dispatched and no reliable terminal result is available, such as connection loss after a mutation. The terminal result has `is_error: true` and code `unknown`; its evidence Event retains the structured transport cause. Timeout and cancellation describe why waiting ended and do not promise rollback. The bridge never automatically repeats a call.

The original input JSON Schema is carried into Brain unchanged through Zod metadata. Host input parsing delegates to the MCP SDK's AJV validator without stripping properties, coercing values or inserting defaults. Conditional and dependent schemas, unevaluated properties, `$defs` and local recursive references are supported when accepted by both MCP and Brain validators. Invalid input fails as `invalid_input` before any MCP call. Unsupported dialects or unresolved references fail at construction; the bridge does not fetch external schemas. The MCP SDK validates structured output against the pinned original definition.

On application restart, reconnect the MCP client and recreate the same selected Tool bindings when reattaching the Brain host. This accepts future commands and never replays a disconnected invocation. Transport credentials remain application-owned; do not put them in Tool arguments or projection metadata.

## Verification

`npm test --workspace @aexhq/tools-mcp` uses the real MCP v2 SDK and a separate stdio server process, including paginated discovery, cancellation, server termination after a mutation, structured results/errors, catalogue changes and images. Public-SDK journeys verify terminal outcomes, journal evidence, model presentation and original-schema validation through Brain, including that invalid calls produce no remote effects.
