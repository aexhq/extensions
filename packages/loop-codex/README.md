# @aexhq/loop-codex

The official codex-style Agentloop for Brain sessions: a semantic port of codex loop policy
including the environment preamble, sequential tool execution, resident conversation cache,
and one summary mark per turn.

```js
import { codex } from "@aexhq/loop-codex";

const agentLoop = codex();
```

`codex()` returns an immutable AgentLoop value. Its package is admitted automatically when a
session is created; Brain does not compile or rewrite source.
