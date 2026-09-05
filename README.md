# extensions

Official Agentloop, Tool, and Environment extensions for Aex. They use only the public
`@aexhq/brain` extension API.

| package | role |
| --- | --- |
| `@aexhq/agentloop-pi` | Pi-style agent loop with parallel Tool calls |
| `@aexhq/agentloop-codex` | Codex-style agent loop with sequential Tool calls |
| `@aexhq/tools` | Model-visible Tool definitions with Environment-side implementations |
| `@aexhq/env-aws-microvm` | AWS MicroVM Environment driver configuration |

Every placed Agentloop and Tool names its Environment explicitly. The loop packages ship
precompiled WebAssembly Components and run in Brain's built-in Wasmtime Environment; workspace
Tools are interpreted by the external Environment driver selected by the application.

```ts
import { brainWasm } from "@aexhq/brain";
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/agentloop-pi";
import { bash, read } from "@aexhq/tools";

const loopRuntime = brainWasm();
const workspace = awsMicroVm({ region: "eu-west-2" });

const session = await brain.sessions.create({
  agentloop: pi({ env: loopRuntime, contextWindow: 200_000 }),
  model,
  tools: [read({ env: workspace }), bash({ env: workspace })],
});
```

Brain accepts components and opaque driver implementations; it does not compile extension source
or install language packages. Each extension publisher owns its build, while the chosen
Environment owns execution and resource enforcement.

Application-resident Tools use the same public factory with `run`. Their code executes in the
application process, and `ctx.emit` records application-defined events in the session journal.

```ts
import { tool } from "@aexhq/brain";
import { z } from "zod";

const notify = tool({
  name: "notify",
  description: "Send a notification.",
  input: z.object({ message: z.string() }),
  run: async ({ message }, ctx) => {
    await ctx.emit("notification_sent", { message });
    return { delivered: true };
  },
});

const tools = [notify()];
```
