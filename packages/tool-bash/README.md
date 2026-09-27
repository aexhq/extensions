# @aexhq/tool-bash

Run a Bash command in the session Environment workspace.

Requires Node.js 22 or newer, Bash and a writable working directory, and the [Brain client setup](https://aex.dev/brain/docs/quickstart).
Add this tool when creating the session:

```ts
import { bash } from "@aexhq/tool-bash";

const tools = [bash()];
```

By default it runs in your Node application and resolves paths from that process's working
directory. Keep the application connected while the agent uses it. For an isolated Docker
workspace, follow the [local environment setup](../env-local/README.md) and pass
`{ env: workspace }`. The factory accepts no other configuration options.

Run `npm test --workspace @aexhq/tool-bash` from the repository root.
