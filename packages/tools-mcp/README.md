# @aexhq/tools-mcp

Connect selected MCP Tools to Brain 0.22 through application `hostEnv`. The application owns the MCP connection, authentication and lifecycle. The bridge never accepts arbitrary model-selected Tool names.

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

Every returned MCP result is first committed as an `mcp_result` Event with Tool provenance. Successful text, structured content and resource links are presented to the model. PNG, JPEG, GIF and WebP images use the official `aex_tool_output` version 1 convention; Pi/Codex 5.1.0 map them into Brain Tool-result media. Audio, binary embedded resources, asynchronous tasks and interactive continuations are explicitly unsupported.

An optional `project(output, toolName)` selects a smaller model-visible JSON representation. Its input includes `content`, optional `structuredContent`, and the committed `evidenceSequence`; raw evidence remains in the Event. Image media is carried separately. Brain's existing emitted-Event and model-input limits still apply; exceeding them is an explicit failure.

MCP `isError` becomes a failed Brain Tool result with code `mcp_tool_error`, `retryable: false` and the complete MCP result in `details`. JSON-RPC failures use `mcp_protocol_error` and retain the original numeric code and data in `details`. Known MCP SDK failures use `mcp_sdk_error`; unsupported continuations use `mcp_unsupported_result`. Protocol/transport errors also produce `mcp_failure` evidence while the invocation is active.

Invocation deadlines and MCP request timeouts produce `timeout`; explicit SDK cancellation produces `cancelled`. The bridge returns `unknown` only when an invocation may have been dispatched and no reliable terminal result is available, such as connection loss after a mutation. The terminal result has `is_error: true` and code `unknown`; its evidence Event retains the structured transport cause. Timeout and cancellation describe why waiting ended and do not promise rollback. The bridge never automatically repeats a call.

The original input JSON Schema is carried into Brain unchanged through Zod metadata. Host input parsing delegates to the MCP SDK's AJV validator without stripping properties, coercing values or inserting defaults. Conditional and dependent schemas, unevaluated properties, `$defs` and local recursive references are supported when accepted by both MCP and Brain validators. Invalid input fails as `invalid_input` before any MCP call. Unsupported dialects or unresolved references fail at construction; the bridge does not fetch external schemas. The MCP SDK validates structured output against the pinned original definition.

On application restart, reconnect the MCP client and recreate the same selected Tool bindings when reattaching the Brain host. This accepts future commands and never replays a disconnected invocation. Transport credentials remain application-owned; do not put them in Tool arguments or projection metadata.

## Verification

`npm test --workspace @aexhq/tools-mcp` uses the real MCP v2 SDK and a separate stdio server process, including paginated discovery, cancellation, server termination after a mutation, structured results/errors, catalogue changes and images. Public-SDK journeys verify terminal outcomes, journal evidence, model presentation and original-schema validation through Brain, including that invalid calls produce no remote effects.
