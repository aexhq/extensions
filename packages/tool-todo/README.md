# @aexhq/tool-todo

Read or replace the session's to-do list.

```js
import { environment } from "@aexhq/brain";
import { todo } from "@aexhq/tool-todo";

const workspace = environment({ url: () => process.env.ENVIRONMENT_URL })({ name: "workspace" });
const tools = [todo({ env: workspace })];
```

Requires a writable workspace. The chosen Environment prepares and executes the
exact published package through its `./runtime` export. The Environment owns isolation and resource grants.
Omit `env` to run in the registering Node application; pass `{ env: workspace }` for an isolated prepared Environment. The factory accepts no configuration options.

Run `npm test --workspace @aexhq/tool-todo` from the repository root.

`todo` rejects competing writes with an explicit conflict and returns the last committed list
on reads. An interrupted writer may leave `.aex/todo.pending`; inspect the list and confirm no
writer is active before removing it. The Agentloop decides whether to retry.
