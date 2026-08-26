# extensions

Official Agentloop, Tool, Environment, and Model components for
[Aex](https://aex.dev). They use the same public `@aexhq/brain` component contract
available to external authors; no package in this repository is privileged.

## Packages

| package | what it is |
| --- | --- |
| `@aexhq/agentloop` | Agentloop authoring and component builder |
| `@aexhq/loop-pi` | pi's agent loop as an Agentloop component |
| `@aexhq/loop-codex` | Codex-style policy as an Agentloop component |
| `@aexhq/tools` | file, shell, search, web, and child-session Tool components |
| `@aexhq/env-app` | customer-application callback Environment component |
| `@aexhq/env-aws-microvm` | AWS Lambda MicroVM Environment component |
| `@aexhq/model` | The typed-failure guard for component exports |

Package factories return immutable component declarations for Brain. Published
packages include precompiled WebAssembly components and their verified identity;
Brain does not compile extension source at runtime. Authors can use another
compiler or language as long as the resulting component implements the public
WIT contract.

## Layout rules

- The four component kinds remain independently selectable and replaceable.
- Tool implementations receive only their declared Brain grants, such as an
  Environment or the bounded child-session interface.
- CI and package smoke tests consume published `@aexhq/brain`; no source-repository
  link is part of the public contract.
