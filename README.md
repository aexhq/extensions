# extensions

Official agent loop, Tool, and Environment extensions for Aex. Every package uses the public
`@aexhq/brain` authoring API and the same `brain build` pipeline; official extensions have no
privileged runtime path.

| package | role |
| --- | --- |
| `@aexhq/agentloop-pi` | Pi-style agent loop with parallel Tool calls |
| `@aexhq/agentloop-codex` | Codex-style agent loop with sequential Tool calls |
| `@aexhq/tools` | Model-visible Tool definitions with provisioned ESM implementations |
| `@aexhq/env-aws-microvm` | AWS MicroVM Environment and provider runtime |

All three roles have the same authoring shape:

```ts
import { agentloop } from "@aexhq/brain";

export const support = agentloop((author) => {
  author.on.message((_message, turn) => turn.reply("Hello"));
});
```

```sh
brain build
```

Agent loop handlers synchronously choose the next durable action.

Tools and Environments meet through the capability contract. A Tool declares `requires` and
programs against typed handles (`context.exec`, `context.fs`, …); an Environment implements
capability providers (`provide.exec`, `provide.fs`, …), enforces grant policy behind them, and
opts in to hosting provisioned ESM artifacts with `host.esm()`. Brain checks
`requires ⊆ provides` at session create and never sees inside either half:

```ts
export const bash = tool({
  description: "Run a shell command.",
  input, output,
  requires: ["exec"],
}, (author) => {
  author.run((input, context) => context.exec.run(input.command));
});
```

Applications place a Tool explicitly with the factory's `env` option —
`bash({ env: environment })` — and can call extension-owned methods on the same Environment
object. A tool whose function lives in an application process needs no environment at all:
declare it with `tool(...)` (with `execute` to run beside the session's creator, without one
to be served by whatever process joins the session with its share key) — Brain routes those
invocations itself.
