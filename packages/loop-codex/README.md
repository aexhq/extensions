# @aexhq/agentloop-codex

A Codex-style agent loop for Brain: a semantic port of the Codex agent loop,
pinned against [openai/codex](https://github.com/openai/codex) tag
`rust-v0.151.0` (= npm `@openai/codex@0.151.0`). The published packages cannot be imported here —
`@openai/codex` ships a precompiled Rust binary and `@openai/codex-sdk` spawns it as a subprocess —
so this package ships a WebAssembly Component that reproduces the loop contract through Brain's
Agentloop host imports:

- each sampling step re-sends the full history; tool calls execute **one at a time** and all
  outputs are appended in the original call order before the next sampling step;
- the turn ends when a response carries no tool calls;
- **automatic compaction at 90% of the context window** uses a summarization model call, then
  retains recent plain user messages and a bridge message carrying the summary.

Codex's remote/server-side compaction, TokenBudget feature, MCP hooks, steering queue, and sandbox
machinery are host concerns and are not ported.

```ts
import { brainEnv } from "@aexhq/brain";
import { codex } from "@aexhq/agentloop-codex";

const loopRuntime = brainEnv({ name: "brain" });
const session = await brain.sessions.create({
  agentloop: codex({ env: loopRuntime, contextWindow: 200_000 }),
  model,
  tools: [bash({ env: workspace })],
});
```

The component is built by this package's publisher. Brain consumes the resulting Component and
does not compile its JavaScript source.

The loop reads paginated session Events before each turn and saves its observation cursor in
kv after saving those observations in the transcript. For unanswered calls in the last saved
assistant message, it inserts error Tool results explaining that the turn was interrupted and
the operation may have run. Saved results, media and native provider state are preserved.
Interrupted turns and environment failures also enter the transcript as runtime observations.
A failed tool result goes back to the model with `is_error`; the loop does not retry it automatically.
Brain can release execution between turns without losing this transcript or cursor. The caller
controls Environment lifetime; a browser closure or provider resource loss can still destroy its
physical state.

By default the model sees canonical Tool schemas. A Tool with multiple placements requires an
explicit `placements: { toolName: "environmentName" }` option. Set
`environmentSelection: "model"` to expose each Tool's authorized Environment names in its input
schema. The loop unwraps that choice before dispatch; Brain validates the selected pair and the
canonical input. Both modes use the same session Tool placements.

Transcript changes and `ctx.kv.read/put/delete` use Brain's state services. KV mutations are
committed inline; missing keys remain distinct from stored JSON null. A later model or
Tool failure preserves acknowledged writes; turn output contains only the result. Retained
native model blocks pass through unchanged, and user images enter model context as media.
Successful Tool outputs shaped as
`{ type: "aex_tool_output", version: 1, content, media: [{ type: "image", url }] }`
become Tool-result content and native media. Images and PDFs require HTTPS URLs. Browser and MCP
extensions use this presentation convention; ordinary JSON/base64 remains business data.
For hosted applications, publish bytes with Aex's attachment API and return its media URL.
Compaction explicitly resets the response format and installs a summary only after `end_turn`;
a truncated, refused, or unknown summary leaves the original saved context intact.

Version 6.1 targets Brain SDK 0.24 and preserves the 0.23 WIT and URL-media contract. Existing sessions keep their
immutable loop implementation; create new sessions to adopt the updated loop. Keep matching
server/artifacts for recovery. An upgrade does not migrate or delete session data.
