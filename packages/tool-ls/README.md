# @aexhq/tool-ls

List entries in an Environment workspace directory.

Requires Node.js 22 or newer, a readable working directory, and the [Brain client setup](https://aex.dev/brain/docs/quickstart).
Add this tool when creating the session:

```ts
import { ls } from "@aexhq/tool-ls";

const tools = [ls()];
```

By default it runs in your Node application and resolves paths from that process's working
directory. Keep the application connected while the agent uses it. For an isolated Docker
workspace, follow the [local environment setup](../env-local/README.md) and pass
`{ env: workspace }`. The factory accepts no other configuration options.

Run `npm test --workspace @aexhq/tool-ls` from the repository root.
