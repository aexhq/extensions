# `@aexhq/tools`

Explicit execution-capability selections for Aex sessions. Nothing effectful is granted by default;
the SDK's reserved structured-output protocol is inert unless one `send({ output })` request arms it.

```ts
import { Aex } from "@aexhq/sdk";
import { bash, edit, read, subagents, write } from "@aexhq/tools";
import { awsMicrovm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/loop-pi";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const workspace = awsMicrovm();
const session = await aex.sessions.create({
  model: {
    provider: "openai",
    name: "gpt-5.4",
    apiKey: process.env.OPENAI_API_KEY!,
  },
  loop: pi(),
  environments: { workspace },
  tools: [bash(), read(), write(), edit(), subagents()],
});
```

`glob()`, `grep()`, `ls()`, and `todo()` are separate opt-ins. Durable storage remains available to
the application through `session.storage`; environment lifecycle is available through the typed
handle returned by `session.environment(workspace)`.

`subagents()` operates durable direct child sessions with explicit spawn, message, follow-up, wait,
list, interrupt and end actions. It is a prepared Tool bound to the same environment rules as every
other Tool. Hosted Aex injects a tenant-fixed session API credential; it cannot authenticate as a
different tenant. Self-hosted deployments provide the equivalent integration.
