# @aexhq/loop-pi

The official pi Agentloop for Brain sessions, adapted from pinned
`@earendil-works/pi-agent-core` onto the canonical Agentloop world. Brain owns provider
execution, credentials and journaling; pi owns loop policy.

```js
import { pi } from "@aexhq/loop-pi";

const agentloop = pi({
  instructions: "Work carefully and verify changes.",
  reasoningEffort: "high",
});
```

`pi(options)` returns an immutable Brain component declaration. The npm package contains
`dist/loop.component.wasm`, compiled before publication through the public
`@aexhq/agentloop/build` API. Options remain configuration data and do not alter executable
bytes. The StarlingMonkey compatibility transforms live in `src/compat.mjs` and affect build
provenance only.
