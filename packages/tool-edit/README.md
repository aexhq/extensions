# @aexhq/tool-edit

Replace one exact occurrence of text in an Environment workspace file.

```js
import { environment } from "@aexhq/brain";
import { edit } from "@aexhq/tool-edit";

const workspace = environment({ url: () => process.env.ENVIRONMENT_URL })({ name: "workspace" });
const tools = [edit({ env: workspace })];
```

Requires a writable workspace. The chosen Environment prepares and executes the
packaged Node 22 runtime in `dist/runtime/edit.mjs`; `dist/runtime/registry.json`
contains its manifest and contract digest. The Environment owns isolation and resource grants.
The factory requires an explicit `env` and accepts no other options.

Run `npm test --workspace @aexhq/tool-edit` from the repository root.
