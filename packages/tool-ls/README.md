# @aexhq/tool-ls

List entries in an Environment workspace directory.

```js
import { environment } from "@aexhq/brain";
import { ls } from "@aexhq/tool-ls";

const workspace = environment({ url: () => process.env.ENVIRONMENT_URL })({ name: "workspace" });
const tools = [ls({ env: workspace })];
```

Requires a readable workspace. The chosen Environment prepares and executes the
packaged Node 22 runtime in `dist/runtime/ls.mjs`; `dist/runtime/registry.json`
contains its manifest and contract digest. The Environment owns isolation and resource grants.
The factory requires an explicit `env` and accepts no other options.

Run `npm test --workspace @aexhq/tool-ls` from the repository root.
