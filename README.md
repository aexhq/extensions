# extensions

Official Agentloops, remote Environments, and Tool definitions for Brain. Every Agentloop uses the
same public Component pipeline; Brain has no privileged built-in extension path. Tool code runs in
an Environment, never in Brain.

| package | role |
| --- | --- |
| `@aexhq/agentloop` | TypeScript authoring API and `brain-loop` Component builder |
| `@aexhq/loop-pi` | Pi-style parallel-Tool Agentloop package |
| `@aexhq/loop-codex` | Codex-style sequential-Tool Agentloop package |
| `@aexhq/tools` | Model-visible Tool definitions plus Environment-side handlers |
| `@aexhq/env-app` | Language-neutral HTTP Environment lifecycle and Tool runtime |
| `@aexhq/env-aws-microvm` | AWS MicroVM Environment requirement and provider runtime |

An Agentloop author writes a synchronous reducer and builds an opaque portable package:

```ts
import { defineAgentloop } from "@aexhq/agentloop";

export default defineAgentloop({
  step(input) {
    return { context: input.context, decision: { type: "finish" } };
  },
});
```

```sh
brain-loop build loop.mjs --out loop.brain.json
```

The toolchain handles WIT, WebAssembly, and the canonical ABI. Authors do not select Wasmtime or
write host imports. Model access and Tool effects are decisions returned to Brain; the Agentloop has
no filesystem, network, secrets, process, or real-time capability.

Environments implement `setup`, `attach`, `call`, `execute`, `cancel`, `detach`, and `teardown` over
the `environment/v1` HTTP contract. They own the real sandbox, browser, user-machine, or remote Tool
lifecycle. Stable operation IDs and request digests make replay and conflict handling explicit.
