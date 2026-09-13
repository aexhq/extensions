# @aexhq/tool-grep

Search text files in the Environment workspace with ripgrep.

```js
import { environment } from "@aexhq/brain";
import { grep } from "@aexhq/tool-grep";

const workspace = environment({ url: () => process.env.ENVIRONMENT_URL })({ name: "workspace" });
const tools = [grep({ env: workspace })];
```

Requires ripgrep and a readable workspace. The chosen Environment prepares and executes the
packaged Node 22 runtime in `dist/runtime/grep.mjs`; `dist/runtime/registry.json`
contains its manifest and contract digest. The Environment owns isolation and resource grants.
The factory requires an explicit `env` and accepts no other options.

Run `npm test --workspace @aexhq/tool-grep` from the repository root.
