# extensions

Official Brain, Tool, and Environment extensions for Aex. Every package uses the public
`@aexhq/brain` authoring API and the same `brain build` pipeline; official extensions have no
privileged runtime path.

| package | role |
| --- | --- |
| `@aexhq/brain-pi` | Pi-style Brain with parallel Tool calls |
| `@aexhq/brain-codex` | Codex-style Brain with sequential Tool calls |
| `@aexhq/tools` | Model-visible Tool definitions and bundled Node implementations |
| `@aexhq/env-app` | Application-process Environment and HTTP provider adapter |
| `@aexhq/env-aws-microvm` | AWS MicroVM Environment and provider runtime |

All three roles have the same authoring shape:

```ts
import { brain } from "@aexhq/brain";

export const support = brain((author) => {
  author.on.message((_message, turn) => turn.reply("Hello"));
});
```

```sh
brain build
```

Brain handlers synchronously choose the next durable action. Tool and Environment handlers may be
async and use normal libraries supported by their runtime. Applications place a Tool explicitly
with `tool().useIn(environment)` and can call extension-owned methods on the same Environment object.
