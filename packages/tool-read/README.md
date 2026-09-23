# @aexhq/tool-read

Read UTF-8 text from a file in the Environment workspace.

```js
import { environment } from "@aexhq/brain";
import { read } from "@aexhq/tool-read";

const workspace = environment({ url: () => process.env.ENVIRONMENT_URL })({ name: "workspace" });
const tools = [read({ env: workspace })];
```

Requires a readable workspace. The chosen Environment prepares and executes the
exact published package through its `./runtime` export. The Environment owns isolation and resource grants.
Omit `env` to run in the registering Node application; pass `{ env: workspace }` for an isolated prepared Environment. The factory accepts no configuration options.

Run `npm test --workspace @aexhq/tool-read` from the repository root.
