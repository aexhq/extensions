# extensions

Official Agentloop, Tool and Environment extensions for Aex. They use only the public
`@aexhq/brain` extension API.

| package | role |
| --- | --- |
| `@aexhq/agentloop-pi` | Pi-style agent loop with parallel Tool calls |
| `@aexhq/agentloop-codex` | Codex-style agent loop with sequential Tool calls |
| `@aexhq/tools` | Model-visible Tool definitions with Environment-side implementations |
| [`@aexhq/env-local`](packages/env-local/README.md) | Docker workspace Environment with retained files and prepared Python projects |
| [`@aexhq/tools-mcp`](packages/tools-mcp/README.md) | Selected MCP Tools in the application's host Environment |
| [`@aexhq/env-browser`](packages/env-browser/README.md) | Playwright Environment and five browser Tools, including screenshot media |

Every placed Agentloop and Tool names its Environment explicitly. The loop packages ship
precompiled WebAssembly Components and run in Brain's built-in Wasmtime Environment; workspace
Tools require an Environment that executes their implementation descriptors, such as `env-local`.

```ts
import { brainEnv, environment } from "@aexhq/brain";
import { pi } from "@aexhq/agentloop-pi";
import { bash, read } from "@aexhq/tools";

const loopRuntime = brainEnv({ name: "brain" });
const workspace = environment({ url: () => process.env.ENVIRONMENT_URL! })({ name: "workspace" });

const session = await brain.sessions.create({
  agentloop: pi({ env: loopRuntime, contextWindow: 200_000 }),
  model,
  tools: [read({ env: workspace }), bash({ env: workspace })],
});
```

Brain accepts components and opaque driver implementations; it does not compile extension source
or install language packages. Each extension publisher owns its build, while the chosen
Environment owns execution and resource enforcement.

The workspace example requires a running Environment server. `env-local` supplies a Docker
implementation; its operator configures trusted images and workspace grants. `env-browser`
requires an operator-supplied browser launcher and deployment isolation. Hosted Aex currently
supports Brain's Wasm Environment and application `hostEnv` Tools; use standalone Brain for
these HTTP Environments.

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

The current packages use Brain 0.22.0. Pi, Codex and Tools are version 5.1.0; the MCP, Docker and
browser extensions are version 0.1.0. Brain 0.22 preserves the existing KV read/put/delete WIT and
adds direct host Outcomes with structured failures. Deploy the updated runtime with the SDK so
Tool deadlines produce `timeout` and cancellation produces `cancelled`. Unknown remains reserved
for missing reliable results after dispatch. Existing sessions retain their immutable extension
implementations; no session data is migrated or deleted by these packages.

Loops use `ctx.kv.read/put/delete`; acknowledged mutations survive later failures. Extension
factories have no `needs`. The chosen Environment prepares ordinary package dependencies before
imports and execution, and enforces its explicitly configured grants. There is no installer
inside Brain and no automatic placement fallback. See
[ADR-046](https://github.com/aexhq/brain/blob/main/references/adrs/2026-09-09-01-minimal-extension-contract.md).

## Tests

Each loop owns its factory/logic tests and `test/journeys.mjs`; shared journey fixtures register
the same contract scenarios separately for Pi and Codex. Tools owns factory tests, built-runtime
integration tests for all eight Tools, and public-SDK workspace journeys under `packages/tools/test`.
The Tools journeys use a local HTTP Environment fixture that executes the packaged runtimes
and verifies their results through Brain.

Install Docker, bash and ripgrep, then run `npx playwright install-deps chromium` and
`npm run test:prepare` to build fixture images and install Chromium. Run `npm test` for
unit/runtime tests and `npm run package-smoke` for packed consumer composition.
With the pinned Brain server listening on `127.0.0.1:18092`, token `extension-fixture-token`, and
model base URL `http://127.0.0.1:18093/v1`, run `npm run test:journeys`. Each of these six packages
also exposes `npm run test:journeys --workspace <package>`. CI starts the pinned Brain image and
runs every journey. Bash and ripgrep must be installed; their runtime tests fail if unavailable.
Scripted model responses make these release gates deterministic; they do not measure live model quality.

The new journeys exercise Docker workspace recovery and lost replies, MCP evidence and disconnects,
and retained browser state with screenshot media through both compiled loops. Runtime tests also
exercise actual process cancellation and resource loss. No runtime tests are skipped when a
dependency is unavailable.
