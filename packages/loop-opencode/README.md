# @aexhq/agentloop-opencode

Run upstream opencode sessions through Brain's journaled model and Tool services.
The extension owns the adapter, native session integration and upstream dependencies.
The executable requires Bun 1.3.14 or newer; the composed `brain-agentloops` image includes it.

Install `@aexhq/agentloop-opencode@1.0.0`. Register the installed executable when starting Brain:

```sh
export BRAIN_AGENTLOOPS='{"opencode":{"command":"brain-agentloop-opencode"}}'
brain
```

Select it for a conversation:

```ts
import { opencode } from "@aexhq/agentloop-opencode";
const session = await brain.sessions.create({
  model, tools, agentloop: opencode({ contextWindow: 128000 }),
});
```

The helper returns a serializable name and configuration; it does not run agent code.
Use a plain selection with a different registered name when the deployment uses aliases.
Session settings override deployment defaults. Only supported behavior options are accepted;
executables, worker counts and concurrency limits are configured at startup.

`/runtime` exports the implementation for custom process composition with
`serveHarness` from `@aexhq/brain/harness`. The package's tests exercise the actual
upstream runtime, including compaction, continuation, cancellation and journal failures.
See [Brain's harness reference](https://aex.dev/brain/docs/reference/agentloop-contract).

Version 1.0.0 requires Brain 0.41.0. Use a new Brain data directory for this prelaunch
contract change. Interrupted native activations are not replayed.
