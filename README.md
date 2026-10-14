# extensions

Official Agentloop, Tool and Environment extensions for Aex. They use only the public
`@aexhq/brain` extension API.

Environment providers are independent of Aex hosting. For example, env-modal accepts the
application's own Modal client and API credentials; Aex's managed keys, admission and billing
are optional composition outside the extension. See the [standalone Modal example](packages/env-modal/README.md).

| package | role |
| --- | --- |
| [`@aexhq/env`](packages/env/README.md) | Scoped Environment control through an ordinary Tool |
| `@aexhq/agentloop-pi` | Upstream Pi agentloop |
| `@aexhq/agentloop-codex` | Upstream Codex agentloop |
| `@aexhq/agentloop-opencode` | Upstream OpenCode agentloop |
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

Brain consumes installed agentloop executables through its public protocol. This repository
owns the Pi, Codex and OpenCode adapters, pinned upstream runtimes and their tests.
Operators register implementations at startup; sessions select an installed name and settings:

```ts
import { pi } from "@aexhq/agentloop-pi";
const session = await brain.sessions.create({ model, tools, agentloop: pi({ contextWindow: 64_000 }) });
```

`pi()` returns selection data. It does not load Pi in the client. See the
[Pi](packages/loop-pi/README.md), [Codex](packages/loop-codex/README.md), and
[OpenCode](packages/loop-opencode/README.md) startup examples. The composed image
`ghcr.io/aexhq/brain-agentloops:latest` includes all three and pins its Brain base image.
Use an immutable digest from the release's `agentloops-image.json` for deployments.

Tools default to the registering application; pass `{ env }` to place them elsewhere.
Workspace Tools ship browser-safe bindings and native Node executables, and run in a
prepared Node host, Docker workspace or Modal profile with the package runtime.
Prompt-based answer validation and corrective turns belong to downstream applications.

Continue the [Brain quickstart](https://aex.dev/brain/docs/quickstart) with its `brain` client
and `model` configuration. Install the tool packages and add them to the session:

```ts
import { bash } from "@aexhq/tool-bash";
import { read } from "@aexhq/tool-read";

const session = await brain.sessions.create({
  model,
  tools: [read(), bash()],
});
```

Tool publishers own their builds. The chosen Environment loads the implementation and
enforces its resource grants.

Environment services use Brain's public `@aexhq/brain/runtime` helpers for command validation,
authenticated HTTP serving and scoped callbacks. Provider code owns resource allocation,
isolation and cancellation. See [Write an environment](https://aex.dev/brain/docs/guides/write-an-environment)
for a service using the same adapters; there is no separate extension-owned wire schema.

These tools run in the registering Node process, using its working directory. Bash must be
installed, and the process must stay connected while tools are needed. For an isolated workspace,
follow [env-local setup](packages/env-local/README.md), then pass `{ env: workspace }` to each tool.
`env-local` requires an operator-configured image and workspace grants. `env-browser`
requires an operator-supplied browser launcher and deployment isolation. Hosted Aex supports
managed Environments, Brain's Wasm Environment, connected `hostEnv` / `clientBrowser` Tools,
and [Application tools](https://aex.dev/docs/application) served by your authenticated HTTPS
endpoint. Use standalone Brain to deploy other Environment services.

Application-resident Tools use the same public factory with `run`. Their code executes in the
application process, and `ctx.emit` records application-defined events in the session journal.
Tool schemas describe accepted inputs; defaulted arguments are optional and the runtime applies
Zod defaults and transforms before invoking the handler. Ordinary objects strip extra fields;
strict objects reject them. The shared client tool connection suspends after five idle seconds
and reconnects before the same live client starts more work. Set `connectionIdleTimeoutMs: 0`
when other callers or future autonomous events must reach its tools. Call `await brain.close()`
when the application is finished with that client. Use `session.interrupt()` to stop a turn, `end()`
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

These packages consume Brain SDK 0.41.0 and its public contracts. Brain remains independently
usable without this repository or Aex. Upstream harnesses own native persistence and compaction;
Brain owns the journal and commits every mediated model and Tool effect before execution.
Native state lives beside the journal. Failed or interrupted native activations require a new
session and are never replayed automatically.
Brain separates Tool return from completion. Return releases the synchronous caller;
`ctx.emitResult(value)` can supply observations before or after return, and `ctx.finish(value)`
commits the final optional result and completion. Both methods accept `{ content: summary }`
to present text alongside the structured result. `ctx.model({ messages })` makes an independent
request under the Tool's lifetime and the session's model authority; it does not edit the transcript.
Use `return ctx.finish(value)` for ordinary
Tools. A Tool that forgets to finish remains open until its original deadline, cancellation or
Environment loss; without a deadline it can remain open indefinitely.

Results and completion enter the ordered journal before the harness receives them.
Harnesses also consume committed background events. Brain coalesces wakeups for five
milliseconds without delaying commits. Interrupt cancels unfinished Tools even between turns.
Environments use automatic lifecycle by default; applications can override it with
`environment.lifecycle`. Include the ordinary `env` Tool when the model should inspect or
manage authorized Environments. Tools and harnesses receive their own fixed control grants.

The upstream adapters expose a fixed Tool catalogue, with one model-visible name per admitted
placement. They resolve the current Environment instance on each dispatch. The `env` Tool can
manage dynamic resources within its grants; upstream adapters do not add new Tool placements
mid-session. There is no automatic placement fallback.

Packages pin their Brain SDK dependency. Use the same version in applications so extension
bindings share its TypeScript brands. Pi supports HTTPS image and PDF inputs and Tool results;
configure a publication callback for Browser screenshots and MCP binary media. The Codex and
OpenCode adapters currently accept text and require an empty initial transcript.

This release requires a new Brain data directory. Old Wasm loops, session-selected executable
Agentloops and their checkpoints are removed. Deploy the matching Brain runtime, SDK and
extensions together.

## Resource lifetimes

Saving a Brain session does not preserve every resource its tools use. Choose a provider with
the lifetime your application needs:

| Provider | What persists and what ends it |
| --- | --- |
| [Local Docker workspace](packages/env-local/README.md#lifecycle-and-failures) | Files survive tool calls and detach; teardown removes the volume. Host or volume loss can still lose files. |
| [Browser](packages/env-browser/README.md#state-and-cancellation) | Browser state survives turns and detach. Teardown, an active-call cancellation/timeout or browser loss closes it; a controller restart cannot reattach it. |
| [Modal](packages/env-modal/README.md#lifecycle-and-accounting) | The sandbox has a finite expiry. Cancel, detach and teardown stop the whole sandbox; an optional turn-end policy can stop it earlier. |
| [HTTP tools](packages/env-http/README.md) | A tool finishes within one application request. Persist business state and background jobs in your application's own storage. |

## Tests

Install Docker, bash and ripgrep, then run `npx playwright install-deps chromium` and
`npm run test:prepare` to build fixture images and install Chromium. Run `npm test` for
unit/runtime tests and `npm run package-smoke` for packed consumer composition.

CI runs every Tool, Environment and MCP journey against the pinned Brain image with
installed extensions. A mixed-runtime test runs 36 sessions across Pi, Codex and OpenCode
concurrently. For local journeys, start Brain on `127.0.0.1:18092` with token
`extension-fixture-token`, model base URL `http://127.0.0.1:18093/v1`, and `BRAIN_AGENTLOOPS`:

```json
{
  "pi": {"command":"brain-agentloop-pi","workers":2,"configuration":{"contextWindow":200000,"maxTokens":8192,"compaction":{"enabled":false}}},
  "codex": {"command":"brain-agentloop-codex"},
  "opencode": {"command":"brain-agentloop-opencode"}
}
```

Then run `npm run test:journeys`. The tests exercise real Docker workspace recovery, lost
replies, MCP errors and cancellation, retained browser state and screenshot media.
Scripted model responses keep these gates deterministic. Missing runtime dependencies fail
checks. Adapter tests cover native resume, compaction, cancellation and effect mediation.
Brain's own suite tests the neutral protocol, worker pools and journal barriers.

## npm publishing

Each package's npm trusted publisher must allow `npm publish` from GitHub organization
`aexhq`, repository `extensions`, workflow filename `npm-publish.yml`, and environment
`npm-production`. Configuring this relationship requires interactive npm authentication;
bypass-2FA access tokens do not authorize `npm trust` operations. See the
[npm setup guide](https://docs.npmjs.com/trusted-publishers/) and
[authentication requirements](https://docs.npmjs.com/cli/v11/commands/npm-trust/#prerequisites).

The [publish workflow](.github/workflows/npm-publish.yml) requires an immutable release tag,
successful source CI, the real Modal integration check and a successful immutable image
build before staging exact package archives under `next`. Supply that image run's ID when
staging. Promotion reuses those archives and the scanned multi-architecture image without
rebuilding them, then publishes their manifests together in the GitHub release.

For the first image publication, set `brain-agentloops` to Public in GitHub package
settings. The image workflow verifies anonymous pulls before it can become a release.
