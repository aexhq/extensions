# extensions

Official Agentloop, Tool and Environment extensions for Aex. They use only the public
`@aexhq/brain` extension API.

Environment providers are independent of Aex hosting. For example, env-modal accepts the
application's own Modal client and API credentials; Aex's managed keys, admission and billing
are optional composition outside the extension. See the [standalone Modal example](packages/env-modal/README.md).

| package | role |
| --- | --- |
| [`@aexhq/env`](packages/env/README.md) | Scoped Environment control through an ordinary Tool |
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
| [`@aexhq/env-http`](packages/env-http/README.md) | Bounded application tools in existing HTTP or serverless APIs |
| [`@aexhq/env-local`](packages/env-local/README.md) | Docker workspace Environment with retained files and prepared Python projects |
| [`@aexhq/env-modal`](packages/env-modal/README.md) | Finite isolated Modal Sandbox with fixed command profiles and cumulative resource usage |
| [`@aexhq/tools-mcp`](packages/tools-mcp/README.md) | Selected MCP Tools in the application's host Environment |
| [`@aexhq/env-browser`](packages/env-browser/README.md) | Playwright Environment and five browser Tools, including screenshot media |

Agentloops name their Environment explicitly. Tools default to the registering application;
pass `{ env }` to place them elsewhere. The loop packages ship precompiled WebAssembly
Components. Workspace Tools ship browser-safe bindings and native Node executables, and run
in a prepared Node host, Docker workspace or Modal profile with the package runtime.
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
  environmentLifecycle: { default: "automatic" },
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
import { tool } from "@aexhq/brain";
import { z } from "zod";

const notify = tool({
  name: "notify",
  description: "Send a notification.",
  input: z.object({ message: z.string() }),
  run: async ({ message }, ctx) => {
    await ctx.emit("notification_sent", { message });
    return ctx.finish({ delivered: true });
  },
});

const tools = [notify()];
```

## Brain runtime boundary

These packages consume Brain's public SDK and contracts. Brain is independently usable without
this repository or Aex. The loops run in fresh Wasm stores, save transcript and kv inline through
Brain's durable state services, and read interruption/environment Events before asking the model to continue.
Brain separates Tool return from completion. Return releases the synchronous caller;
`ctx.emitResult(value)` can supply observations before or after return, and `ctx.finish(value)`
commits the final optional result and completion. Both methods accept `{ content: summary }`
to present text alongside the structured result. `ctx.model({ messages })` makes an independent
request under the Tool's lifetime and the session's model authority; it does not edit the transcript.
Use `return ctx.finish(value)` for ordinary
Tools. A Tool that forgets to finish remains open until its original deadline, cancellation or
Environment loss; without a deadline it can remain open indefinitely.

Results and completion always enter the ordered journal. Pi and Codex choose their model
messages, save them, then explicitly acknowledge the processed sequence. An event activation
has no user input. Both loops ask the model to consider new background results and completion;
an activation containing only already-consumed events is acknowledged without a model call.
Brain coalesces wakeups for five milliseconds from the first pending event without delaying
commits. Interrupt cancels unfinished Tools even between turns.
Applications explicitly select automatic or manual Environment lifecycle at session creation.
Automatic lifecycle runs setup and cleanup without involving the model. Include the ordinary
`env` Tool separately when the model should inspect or manage authorized Environments.
Agentloops, Tools and Environment operations share `ctx.environments`; each receives its own
fixed grants. Long-lived Environment reporters can publish idle observations without retaining
an execution context. Both loops preserve those observations and their Environment origin. Providers implement
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

For dynamic instances, grant the Agentloop `read` access to the template and select
`environmentSelection: "model"` or an explicit `placements` policy. Both loops refresh
authorized placements and pin each dispatch to the current Environment reference. Pi dispatches
parallel batches: put setup or resource changes before dependent calls in separate batches.

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

## npm publishing

Each package's npm trusted publisher must allow `npm publish` from GitHub organization
`aexhq`, repository `extensions`, workflow filename `npm-publish.yml`, and environment
`npm-production`. Configuring this relationship requires interactive npm authentication;
bypass-2FA access tokens do not authorize `npm trust` operations. See the
[npm setup guide](https://docs.npmjs.com/trusted-publishers/) and
[authentication requirements](https://docs.npmjs.com/cli/v11/commands/npm-trust/#prerequisites).

The [publish workflow](.github/workflows/npm-publish.yml) requires an immutable release tag,
successful source CI and the real Modal integration check before staging exact package
archives under `next`. Promotion reuses those archives without rebuilding them.
