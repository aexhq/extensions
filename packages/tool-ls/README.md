# @aexhq/tool-ls

List entries in an Environment workspace directory.

```js
import { environment } from "@aexhq/brain";
import { ls } from "@aexhq/tool-ls";

const workspace = environment({ url: () => process.env.ENVIRONMENT_URL })({ name: "workspace" });
const tools = [ls({ env: workspace })];
```

Requires a readable workspace. The chosen Environment prepares and executes the
exact published package through its `./runtime` export. The Environment owns isolation and resource grants.
Omit `env` to run in the registering Node application; pass `{ env: workspace }` for an isolated prepared Environment. The factory accepts no configuration options.

Run `npm test --workspace @aexhq/tool-ls` from the repository root.
