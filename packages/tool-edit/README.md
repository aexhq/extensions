# @aexhq/tool-edit

Replace one exact occurrence of text in an Environment workspace file.

Requires Node.js 22 or newer, a writable working directory, and the [Brain client setup](https://aex.dev/brain/docs/quickstart).
Add this tool when creating the session:

```ts
import { edit } from "@aexhq/tool-edit";

const tools = [edit()];
```

By default it runs in your Node application and resolves paths from that process's working
directory. Keep the application connected while the agent uses it. For an isolated Docker
workspace, follow the [local environment setup](../env-local/README.md) and pass
`{ env: workspace }`. The factory accepts no other configuration options.

Run `npm test --workspace @aexhq/tool-edit` from the repository root.
