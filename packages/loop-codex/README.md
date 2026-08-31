# @aexhq/agentloop-codex

A Codex-style agent loop for Brain: a semantic port of the Codex agent loop,
pinned against [openai/codex](https://github.com/openai/codex) tag
`rust-v0.151.0` (= npm `@openai/codex@0.151.0`). The published packages cannot
be imported here — `@openai/codex` ships a precompiled Rust binary and
`@openai/codex-sdk` spawns it as a subprocess — so this package reproduces the
loop's contract inside Brain's deterministic WebAssembly sandbox:

- each step re-sends the full history; tool calls execute **one at a time**
  (Codex's default per-tool gate is exclusive) and all outputs are appended in
  the **original call order** before the next sampling step;
- the turn ends when a response carries no tool calls;
- **automatic compaction at 90% of the context window**, following Codex's
  local path: a summarization model call (Codex's own compaction prompt), then
  history is replaced by the prior plain user messages (up to ~20k tokens,
  most recent kept) plus one bridge message carrying the summary. Token usage
  comes from the provider's reported counts, with a client-side estimate as
  the fallback.

Codex's remote/server-side compaction, TokenBudget feature, MCP hooks,
steering queue, and sandbox machinery are host concerns and are not ported.

```ts
import { codex } from "@aexhq/agentloop-codex";

const session = await brain.sessions.create({
  agentloop: codex({ contextWindow: 200_000 }),
  model,
  tools: [bash().useIn(workspace)],
});
```
