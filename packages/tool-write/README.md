# @aexhq/tool-write

Write UTF-8 text to a file in the Environment workspace, creating parent directories.

```js
import { environment } from "@aexhq/brain";
import { write } from "@aexhq/tool-write";

const workspace = environment({ url: () => process.env.ENVIRONMENT_URL })({ name: "workspace" });
const tools = [write({ env: workspace })];
```

Requires a writable workspace. The chosen Environment prepares and executes the
packaged Node 22 runtime in `dist/runtime/write.mjs`; `dist/runtime/registry.json`
contains its manifest and contract digest. The Environment owns isolation and resource grants.
The factory requires an explicit `env` and accepts no other options.

Run `npm test --workspace @aexhq/tool-write` from the repository root.
