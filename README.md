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
import { brainEnv } from "@aexhq/brain";
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/agentloop-pi";
import { bash, read } from "@aexhq/tools";

const loopRuntime = brainEnv({ name: "brain" });
const workspace = awsMicroVm({ name: "sandbox", url: process.env.ENVIRONMENT_URL, token: process.env.ENVIRONMENT_TOKEN, region: "eu-west-2" });

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
import { hostEnv, tool } from "@aexhq/brain";
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

const tools = [notify({ env: hostEnv({ name: "app" }) })];
```

## Brain runtime boundary

These packages consume Brain's public SDK and contracts. Brain is independently usable without
this repository or Aex. The loops run in fresh Wasm stores, save transcript and kv inline through
Brain's durable state services, and read interruption/environment Events before asking the model to continue.
Callers control Environment lifetime through setup, detach, and teardown. Providers implement
those operations and enforce their physical resource ceilings. A Tool can be declared in several
named Environments; the loop either selects a configured placement or presents authorized choices
to the model. See the loop packages' `placements` and `environmentSelection` options.

Version 4 packages use Brain 0.19.0 and its new Agentloop WIT. Update the server alongside the SDK
and extensions, and recreate sessions with the rebuilt loop Components. A turn returns only its
result; acknowledged state writes survive later failures. See
[ADR-045](https://github.com/aexhq/brain/blob/main/references/adrs/2026-09-07-01-protocol-freeze.md)
for the protocol changes, compatibility limits, and deferred work.

## Tests

Each loop owns its factory/logic tests and `test/journeys.mjs`; shared journey fixtures register
the same contract scenarios separately for Pi and Codex. Tools owns factory tests, built-runtime
integration tests for all eight Tools, and public-SDK workspace journeys under `packages/tools/test`.
The Tools journeys use a local HTTP Environment fixture that executes the packaged runtimes;
they do not provision AWS or validate MicroVM isolation.

Run `npm test` for unit/runtime tests and `npm run package-smoke` for packed consumer composition.
With the pinned Brain server listening on `127.0.0.1:18092`, token `extension-fixture-token`, and
model base URL `http://127.0.0.1:18093/v1`, run `npm run test:journeys`. Each of these three packages
also exposes `npm run test:journeys --workspace <package>`. CI starts the pinned Brain image and
runs every journey. Bash and ripgrep must be installed; their runtime tests fail if unavailable.
Scripted model responses make these release gates deterministic; they do not measure live model quality.
