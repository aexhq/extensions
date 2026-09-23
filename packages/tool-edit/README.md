# @aexhq/tool-edit

Replace one exact occurrence of text in an Environment workspace file.

```js
import { environment } from "@aexhq/brain";
import { edit } from "@aexhq/tool-edit";

const workspace = environment({ url: () => process.env.ENVIRONMENT_URL })({ name: "workspace" });
const tools = [edit({ env: workspace })];
```

Requires a writable workspace. The chosen Environment prepares and executes the
exact published package through its `./runtime` export. The Environment owns isolation and resource grants.
Omit `env` to run in the registering Node application; pass `{ env: workspace }` for an isolated prepared Environment. The factory accepts no configuration options.

Run `npm test --workspace @aexhq/tool-edit` from the repository root.
