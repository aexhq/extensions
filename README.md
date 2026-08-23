# extensions

Official extensions for the [Aex](https://aex.dev) platform, as ordinary
packages: agentloops today, tool packages next. Everything here builds only
against the **published** public SDKs (`@aexhq/agentloop`, `@aexhq/sdk`) — the
same surface any external contributor uses. Nothing in this repository is
privileged; the hosted composition seeds these artifacts through the ordinary
admission path.

## Packages

| package | what it is |
| --- | --- |
| `@aexhq/loop-aex` | the official default agentloop (hosted default policy) |
| `@aexhq/loop-pi` | pi's agent loop adapted onto `contracts/agentloop/v1` |
| `@aexhq/loop-codex` | a codex-style loop policy port |

Each loop package builds a deterministic source bundle plus a sealed identity
(`dist/loop.bundle.mjs` + `dist/identity.json`) via `buildLoopBundle` from
`@aexhq/agentloop`, and default-exports the `{ source, sha256, toolchain }`
object that `sessions.create({ agentloop })` accepts.

## Layout rules

- Loops and tools are separate packages; officials are not coupled to each other
  or to the kernel.
- CI installs dependencies from the npm registry only — no git links into the
  kernel repo.
