# @aexhq/agentloop-pi

Run upstream pi sessions through Brain's journaled model and Tool services.
The extension owns the adapter, native session integration and upstream dependencies.

Install `@aexhq/agentloop-pi@11.0.0`. Register the installed executable when starting Brain:

```sh
export BRAIN_AGENTLOOPS='{"pi":{"command":"brain-agentloop-pi"}}'
brain
```

Select it for a conversation:

```ts
import { pi } from "@aexhq/agentloop-pi";
const session = await brain.sessions.create({
  model, tools, agentloop: pi({ contextWindow: 128000 }),
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

Version 11.0.0 requires Brain 0.41.0. Use a new Brain data directory for this prelaunch
contract change. Interrupted native activations are not replayed.
