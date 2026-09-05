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
import { brainWasm } from "@aexhq/brain";
import { codex } from "@aexhq/agentloop-codex";

const loopRuntime = brainWasm();
const session = await brain.sessions.create({
  agentloop: codex({ env: loopRuntime, contextWindow: 200_000 }),
  model,
  tools: [bash({ env: workspace })],
});
```

The component is built by this package's publisher. Brain consumes the resulting Component and
does not compile its JavaScript source.
