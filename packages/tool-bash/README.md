# @aexhq/tool-bash

Run a Bash command in the session Environment workspace.

```js
import { environment } from "@aexhq/brain";
import { bash } from "@aexhq/tool-bash";

const workspace = environment({ url: () => process.env.ENVIRONMENT_URL })({ name: "workspace" });
const tools = [bash({ env: workspace })];
```

Requires bash and a writable workspace. The chosen Environment prepares and executes the
packaged Node 22 runtime in `dist/runtime/bash.mjs`; `dist/runtime/registry.json`
contains its manifest and contract digest. The Environment owns isolation and resource grants.
The factory requires an explicit `env` and accepts no other options.

Run `npm test --workspace @aexhq/tool-bash` from the repository root.
