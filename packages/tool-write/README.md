# @aexhq/tool-write

Write UTF-8 text to a file in the Environment workspace, creating parent directories.

Requires Node.js 22 or newer, a writable working directory, and the [Brain client setup](https://aex.dev/brain/docs/quickstart).
Add this tool when creating the session:

```ts
import { write } from "@aexhq/tool-write";

const tools = [write()];
```

By default it runs in your Node application and resolves paths from that process's working
directory. Keep the application connected while the agent uses it. For an isolated Docker
workspace, follow the [local environment setup](../env-local/README.md) and pass
`{ env: workspace }`. The factory accepts no other configuration options.

Run `npm test --workspace @aexhq/tool-write` from the repository root.
