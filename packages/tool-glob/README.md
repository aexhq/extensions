# @aexhq/tool-glob

List Environment workspace paths matching a glob pattern.

```js
import { environment } from "@aexhq/brain";
import { glob } from "@aexhq/tool-glob";

const workspace = environment({ url: () => process.env.ENVIRONMENT_URL })({ name: "workspace" });
const tools = [glob({ env: workspace })];
```

Requires a readable workspace. The chosen Environment prepares and executes the
exact published package through its `./runtime` export. The Environment owns isolation and resource grants.
Omit `env` to run in the registering Node application; pass `{ env: workspace }` for an isolated prepared Environment. The factory accepts no configuration options.

Run `npm test --workspace @aexhq/tool-glob` from the repository root.
