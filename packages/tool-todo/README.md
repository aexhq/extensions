# @aexhq/tool-todo

Read or replace a workspace to-do list.

Requires Node.js 22 or newer, a writable working directory, and the [Brain client setup](https://aex.dev/brain/docs/quickstart).
Add this tool when creating the session:

```ts
import { todo } from "@aexhq/tool-todo";

const tools = [todo()];
```

By default it runs in your Node application and resolves paths from that process's working
directory. Keep the application connected while the agent uses it. For an isolated Docker
workspace, follow the [local environment setup](../env-local/README.md) and pass
`{ env: workspace }`. The factory accepts no other configuration options.

Run `npm test --workspace @aexhq/tool-todo` from the repository root.

`todo` stores the list in `.aex/todo.json`. Sessions using the same working directory share
that list; use separate working directories or environments for separate lists.

`todo` rejects competing writes with an explicit conflict and returns the last committed list
on reads. An interrupted writer may leave `.aex/todo.pending`; inspect the list and confirm no
writer is active before removing it. The Agentloop decides whether to retry.
