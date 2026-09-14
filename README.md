# extensions

Official Agentloop, Tool and Environment extensions for Aex. They use only the public
`@aexhq/brain` extension API.

| package | role |
| --- | --- |
| `@aexhq/agentloop-pi` | Pi-style agent loop with parallel Tool calls |
| `@aexhq/agentloop-codex` | Codex-style agent loop with sequential Tool calls |
| [`@aexhq/tool-bash`](packages/tool-bash/README.md) | Run Bash commands |
| [`@aexhq/tool-edit`](packages/tool-edit/README.md) | Replace one exact text occurrence |
| [`@aexhq/tool-glob`](packages/tool-glob/README.md) | Find workspace paths by glob pattern |
| [`@aexhq/tool-grep`](packages/tool-grep/README.md) | Search workspace text with ripgrep |
| [`@aexhq/tool-ls`](packages/tool-ls/README.md) | List directory entries |
| [`@aexhq/tool-read`](packages/tool-read/README.md) | Read UTF-8 text |
| [`@aexhq/tool-todo`](packages/tool-todo/README.md) | Read or replace the workspace to-do list |
| [`@aexhq/tool-write`](packages/tool-write/README.md) | Write UTF-8 files |
| [`@aexhq/env-local`](packages/env-local/README.md) | Docker workspace Environment with retained files and prepared Python projects |
| [`@aexhq/env-modal`](packages/env-modal/README.md) | Finite isolated Modal Sandbox with fixed command profiles and cumulative resource usage |
| [`@aexhq/tools-mcp`](packages/tools-mcp/README.md) | Selected MCP Tools in the application's host Environment |
| [`@aexhq/env-browser`](packages/env-browser/README.md) | Playwright Environment and five browser Tools, including screenshot media |

Every placed Agentloop and Tool names its Environment explicitly. The loop packages ship
precompiled WebAssembly Components and run in Brain's built-in Wasmtime Environment; workspace
Tools require an Environment that executes their implementation descriptors, such as `env-local`.
Both official loops support hosted JSON Schema output correction in the same turn, including
turns started with `session.submit()`. Configure `output: { schema, maxCorrections: 2 }` on the loop;
correction requests have no tools and only a valid final answer emits `assistant_message`.

```ts
import { brainEnv, environment } from "@aexhq/brain";
import { pi } from "@aexhq/agentloop-pi";
import { bash } from "@aexhq/tool-bash";
import { read } from "@aexhq/tool-read";

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
requires an operator-supplied browser launcher and deployment isolation. Hosted Aex admits
its published managed Environment catalog, Brain's Wasm Environment and application `hostEnv`
Tools. Use standalone Brain for your own HTTP Environment endpoints.

Application-resident Tools use the same public factory with `run`. Their code executes in the
application process, and `ctx.emit` records application-defined events in the session journal.
Tool schemas describe accepted inputs; defaulted arguments are optional and the runtime applies
Zod defaults and transforms before invoking the handler. Ordinary objects strip extra fields;
strict objects reject them. Put client use, including session creation, inside `try/finally`
and call `await brain.close()` when finished. Use `session.interrupt()` to stop a turn, `end()`
to finish a conversation, and `delete()` to remove its stored history.

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

Packages pin their Brain SDK dependency in their manifests. Images and PDFs use HTTPS URLs in user input and Tool results.
Configure a publication callback for Browser screenshots and MCP binary media. Pi preserves native
media during summarization. Deploy the matching Brain runtime, SDK and extensions together; retained
sessions keep their immutable implementations and require a compatibility check before upgrading.

Loops use `ctx.kv.read/put/delete`; acknowledged mutations survive later failures. Extension
factories have no `needs`. The chosen Environment prepares ordinary package dependencies before
imports and execution, and enforces its explicitly configured grants. There is no installer
inside Brain and no automatic placement fallback. See
[ADR-046](https://github.com/aexhq/brain/blob/main/references/adrs/2026-09-09-01-minimal-extension-contract.md).

## Tests

For quick Pi policy edits, use `npm run test:logic:watch -w packages/loop-pi`; it runs the existing
logic tests without a Component build. Full tests and compiled journeys still gate release.

Each loop owns its factory/logic tests and `test/journeys.mjs`; shared journey fixtures register
the same contract scenarios separately for Pi and Codex. Each `packages/tool-*` package owns its
factory, schemas, runtime and tests; it builds and publishes without depending on another Tool.
Public-SDK workspace journeys under `test/tools-journeys.mjs` compose all eight packages through
a local HTTP Environment fixture and verify their results through Brain.

Install Docker, bash and ripgrep, then run `npx playwright install-deps chromium` and
`npm run test:prepare` to build fixture images and install Chromium. Run `npm test` for
unit/runtime tests and `npm run package-smoke` for packed consumer composition.
With the pinned Brain server listening on `127.0.0.1:18092`, token `extension-fixture-token`, and
model base URL `http://127.0.0.1:18093/v1`, run `npm run test:journeys`. The loop, Environment and MCP packages
also expose `npm run test:journeys --workspace <package>`. CI starts the pinned Brain image and
runs every journey. Bash and ripgrep must be installed; their runtime tests fail if unavailable.
Scripted model responses make these release gates deterministic; they do not measure live model quality.

The new journeys exercise Docker workspace recovery and lost replies, MCP evidence and disconnects,
and retained browser state with screenshot media through both compiled loops. Runtime tests also
exercise actual process cancellation and resource loss. No runtime tests are skipped when a
dependency is unavailable.
