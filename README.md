# extensions

Official extensions for the [Aex](https://aex.dev) platform, as ordinary
brain, tool, and environment packages. Everything here builds only
against the **published** public SDKs (`@aexhq/agentloop`, `@aexhq/sdk`) — the
same surface any external contributor uses. Nothing in this repository is
privileged; the hosted composition seeds these artifacts through the ordinary
admission path.

## Packages

| package | what it is |
| --- | --- |
| `@aexhq/agentloop` | brain-extension authoring and deterministic bundle builder |
| `@aexhq/loop-pi` | pi's agent loop adapted onto `contracts/agentloop/v1` |
| `@aexhq/loop-codex` | a codex-style loop policy port |
| `@aexhq/tools` | official prepared computer tools |
| `@aexhq/env-app` | application callback environment |
| `@aexhq/env-aws-microvm` | AWS Lambda MicroVM computer environment |

Each loop package builds a deterministic source bundle plus a sealed identity
(`dist/loop.bundle.mjs` + `dist/identity.json`) via `buildLoopBundle` from
`@aexhq/agentloop`, and default-exports the `{ source, sha256, toolchain }`
object that `sessions.create({ loop })` accepts.

## Layout rules

- Loops and tools are separate packages; officials are not coupled to each other
  or to the kernel.
- CI installs dependencies from the npm registry only — no git links into the
  kernel repo.
