# @aexhq/loop-aex

The official aex agentloop — the default hosted policy, written on
`contracts/agentloop/v1` with the public `@aexhq/agentloop` SDK exactly as any
customer loop is. Sessions that omit an agentloop get this policy; importing it
makes the assignment explicit:

```js
import aexLoop from "@aexhq/loop-aex";

const session = await aex.sessions.create({ model, agentloop: aexLoop });
```

Policy highlights: sealed presentation verbatim (frozen provider base and prompt
cache reuse every ordinary round), parallel tool-batch dispatch, one graceful
`tool_choice: "none"` closing round at the sealed round ceiling, a summary mark
per turn, and loop-side compaction on budget exhaustion.

The in-kernel twin is `BuiltinAexLoop` in `aexhq/brain` — policy changes land in
both.
