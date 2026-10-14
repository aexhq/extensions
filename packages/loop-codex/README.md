# @aexhq/agentloop-codex

Run the upstream Codex agent through Brain's model adapter and authorized Tools.
The upstream agent owns its conversation loop, compaction and native session state.
This package exports Brain's Codex harness and installs its pinned upstream runtime.

Install `@aexhq/brain@0.40.0` and `@aexhq/agentloop-codex@10.0.0` in the server deployment.
Create `harness.mjs`:

```js
import { serveHarness } from "@aexhq/brain/harness";
import { codex } from "@aexhq/agentloop-codex";

await serveHarness(codex());
```

Start Brain with that implementation:

```sh
brain --harness '{"command":"node","args":["/absolute/path/harness.mjs"]}'
```

Clients create sessions with `model`, `tools` and optional `harness.configuration`.
The deployment selects one harness before readiness; session callers cannot choose executable code.
Configuration is documented in Brain's [harness reference](https://aex.dev/brain/docs/reference/agentloop-contract).
Models without context metadata need explicit token limits in harness configuration.

Version 10 replaces the previous handwritten Wasm loop. Start with a new Brain data directory;
old loop placement, output correction, dynamic tool catalogues and checkpoints are unsupported.
The Codex adapter accepts text and requires an empty initial transcript.
Interrupted native activations are not replayed: create a new session after an interrupted or failed run.
Model and Tool effects remain journaled by Brain before the adapter receives their results.

The complete upstream adapter tests live in [Brain](https://github.com/aexhq/brain/tree/main/packages/brain-sdk/test).
This package adds no loop policy or compaction implementation.
