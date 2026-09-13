# @aexhq/tool-glob

List Environment workspace paths matching a glob pattern.

```js
import { environment } from "@aexhq/brain";
import { glob } from "@aexhq/tool-glob";

const workspace = environment({ url: () => process.env.ENVIRONMENT_URL })({ name: "workspace" });
const tools = [glob({ env: workspace })];
```

Requires a readable workspace. The chosen Environment prepares and executes the
packaged Node 22 runtime in `dist/runtime/glob.mjs`; `dist/runtime/registry.json`
contains its manifest and contract digest. The Environment owns isolation and resource grants.
The factory requires an explicit `env` and accepts no other options.

Run `npm test --workspace @aexhq/tool-glob` from the repository root.
