# @aexhq/agentloop-pi

A pi-style agent loop for Brain: a semantic port of the pi coding agent's loop,
pinned against [earendil-works/pi](https://github.com/earendil-works/pi) tag
`v0.84.4` (`@earendil-works/pi-agent-core@0.84.4`). The package ships a precompiled
WebAssembly Component that drives each turn through Brain's Agentloop host imports and reproduces
pi's per-turn contract:

- tool calls are issued as one **parallel batch**, and results return in assistant source order;
- a `length`-stopped response that carries tool calls **fails the whole batch without executing
  it** and re-asks the model;
- **automatic compaction**: when the estimated context exceeds
  `contextWindow - reserveTokens` (default 16384), history older than ~`keepRecentTokens`
  (default 20000) is summarized into pi's structured context checkpoint.

pi's steering and follow-up queues and per-tool `executionMode` are host-app seams with no Brain
equivalent and are not ported.

```ts
import { brainEnv } from "@aexhq/brain";
import { pi } from "@aexhq/agentloop-pi";

const loopRuntime = brainEnv({ name: "brain" });
const session = await brain.sessions.create({
  agentloop: pi({ env: loopRuntime, contextWindow: 200_000 }),
  model,
  tools: [read({ env: workspace })],
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
Version 5.1 also presents successful Tool outputs shaped as
`{ type: "aex_tool_output", version: 1, content, media: [{ type: "image", url }] }`
as Tool-result content and image media. Browser and MCP extensions use this presentation
convention; ordinary JSON results keep their existing behavior.
Compaction explicitly resets the response format and installs a summary only after `end_turn`;
a truncated, refused, or unknown summary leaves the original saved context intact.

Version 5.1 targets Brain 0.22 and preserves the existing WIT. Existing sessions keep their
immutable loop implementation; create new sessions to adopt the updated loop. Keep matching
server/artifacts for recovery. An upgrade does not migrate or delete session data.
