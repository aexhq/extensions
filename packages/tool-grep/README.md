# @aexhq/tool-grep

Search text files in the Environment workspace with ripgrep.

Requires Node.js 22 or newer, ripgrep and a readable working directory, and the [Brain client setup](https://aex.dev/brain/docs/quickstart).
Add this tool when creating the session:

```ts
import { grep } from "@aexhq/tool-grep";

const tools = [grep()];
```

By default it runs in your Node application and resolves paths from that process's working
directory. Keep the application connected while the agent uses it. For an isolated Docker
workspace, follow the [local environment setup](../env-local/README.md) and pass
`{ env: workspace }`. The factory accepts no other configuration options.

Run `npm test --workspace @aexhq/tool-grep` from the repository root.
