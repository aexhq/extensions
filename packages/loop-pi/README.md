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
import { brainWasm } from "@aexhq/brain";
import { pi } from "@aexhq/agentloop-pi";

const loopRuntime = brainWasm();
const session = await brain.sessions.create({
  agentloop: pi({ env: loopRuntime, contextWindow: 200_000 }),
  model,
  tools: [read({ env: workspace })],
});
```

The component is built by this package's publisher. Brain consumes the resulting Component and
does not compile its JavaScript source.

The loop reads paginated session Events before each turn and saves its observation cursor in
slots. Interrupted turns and environment failures enter the transcript as runtime observations.
A failed tool result goes back to the model with `is_error`; the loop does not retry it automatically.
Brain can release execution between turns without losing this transcript or cursor. Physical
browser and sandbox state follows the Environment provider's TTL and may be lost. The official
`tool-env` inspection and lifecycle tool is planned after the MVP.
