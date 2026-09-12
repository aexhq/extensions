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

Tools and their Environments coordinate access to shared resources with their own transactions,
locks or queues. A lock prevents overlap; dependency-sensitive work still needs ordered dispatch
or a Tool that owns the whole operation. Pi has no per-tool scheduling flag or batch Tool protocol.
Its steering and follow-up queues are host application concerns and are not ported.

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
Results keep call IDs and assistant source order, including mixed image/text batches and failed
siblings. For hosted applications, publish bytes with Aex's attachment API and return its media URL.
Compaction explicitly resets the response format and installs a summary only after `end_turn`;
a truncated, refused, or unknown summary leaves the original saved context intact.

Version 6.1 targets Brain SDK 0.24 and preserves the 0.23 WIT and URL-media contract. Existing sessions keep their
immutable loop implementation; create new sessions to adopt the updated loop. Keep matching
server/artifacts for recovery. An upgrade does not migrate or delete session data.

Image and PDF blocks remain native media in compaction requests. The JSON-length estimate used
for new context does not measure image/PDF tokens; provider context-limit failures remain explicit.

Use `session.send(prompt, { output: { type: AnswerSchema } })` for validated application output.
Pi finishes when the assistant returns no Tool calls; no special terminal Tool is required.
Ordinary `send()` returns session state, so `idle` alone does not establish turn success.

## Develop the loop

From a checkout of this repository, run `npm run test:logic -w packages/loop-pi` or
`npm run test:logic:watch -w packages/loop-pi`. These execute the existing JavaScript logic tests
without rebuilding Wasm. Run `npm test -w packages/loop-pi` for the Component build and package
tests, then the compiled journeys described in the repository README. All release gates remain
required. A rebuilt Component needs a new admission and session. Package source is not a public
logic-library entry point, and existing sessions keep their admitted implementation.
