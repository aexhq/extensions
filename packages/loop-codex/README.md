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
kv. Interrupted turns and environment failures enter the transcript as runtime observations.
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
Compaction explicitly resets the response format and installs a summary only after `end_turn`;
a truncated, refused, or unknown summary leaves the original saved context intact.

Rebuild Components against the matching Brain WIT when upgrading. Existing sessions keep their
immutable loop implementation. Keep their matching server/artifacts for recovery and create
new sessions explicitly when adopting Brain 0.20 / version 5 extensions; do not delete old data
as an implicit upgrade step.
