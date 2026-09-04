# @aexhq/agentloop-pi

A pi-style agent loop for Brain: a semantic port of the pi coding agent's loop,
pinned against [earendil-works/pi](https://github.com/earendil-works/pi) tag
`v0.84.4` (`@earendil-works/pi-agent-core@0.84.4`). The loop drives each turn
through Brain's services from inside its WebAssembly component and reproduces
pi's per-turn contract:

- tool calls are issued as one **parallel batch**, and results return in
  assistant source order;
- a `length`-stopped response that carries tool calls **fails the whole batch
  without executing it** and re-asks the model;
- **automatic compaction**: when the estimated context exceeds
  `contextWindow - reserveTokens` (default 16384), history older than
  ~`keepRecentTokens` (default 20000) is summarized into pi's structured
  context checkpoint (`## Goal` … `## Critical Context`) that replaces it.

pi's steering and follow-up queues and per-tool `executionMode` are host-app
seams with no Brain equivalent and are not ported.

```ts
import { pi } from "@aexhq/agentloop-pi";

const session = await brain.sessions.create({
  agentloop: pi({ contextWindow: 200_000 }),
  model,
  tools: [read({ env: workspace })],
});
```
