# @aexhq/tool-write

Write UTF-8 text to a file in the Environment workspace, creating parent directories.

```js
import { environment } from "@aexhq/brain";
import { write } from "@aexhq/tool-write";

const workspace = environment({ url: () => process.env.ENVIRONMENT_URL })({ name: "workspace" });
const tools = [write({ env: workspace })];
```

Requires a writable workspace. The chosen Environment prepares and executes the
exact published package through its `./runtime` export. The Environment owns isolation and resource grants.
Omit `env` to run in the registering Node application; pass `{ env: workspace }` for an isolated prepared Environment. The factory accepts no configuration options.

Run `npm test --workspace @aexhq/tool-write` from the repository root.
