# @aexhq/loop-codex

The official codex-style Agentloop for Brain sessions: a semantic port of codex loop policy
including the environment preamble, sequential tool execution, resident conversation cache,
and one summary mark per turn.

```js
import { codex } from "@aexhq/loop-codex";

const agentloop = codex({ instructions: "Prefer small, verified changes." });
```

`codex(options)` returns an immutable Brain component declaration. The npm package contains
`dist/loop.component.wasm`, compiled before publication through the public
`@aexhq/agentloop/build` API. Options cross the session boundary as immutable configuration
data; Brain does not compile or rewrite source.
