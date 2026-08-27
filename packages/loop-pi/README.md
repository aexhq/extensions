# @aexhq/loop-pi

The official pi Agentloop for Brain sessions, adapted from pinned
`@earendil-works/pi-agent-core` onto the canonical Agentloop world. Brain owns provider
execution, credentials and journaling; pi owns loop policy.

```js
import { pi } from "@aexhq/loop-pi";

const agentLoop = pi();
```

`pi()` returns an immutable AgentLoop value. Its package is admitted automatically when a session
is created. The StarlingMonkey compatibility transforms live in `src/compat.mjs` and affect build
provenance only.
