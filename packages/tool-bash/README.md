# @aexhq/tool-bash

Run a Bash command in the session Environment workspace.

```js
import { environment } from "@aexhq/brain";
import { bash } from "@aexhq/tool-bash";

const workspace = environment({ url: () => process.env.ENVIRONMENT_URL })({ name: "workspace" });
const tools = [bash({ env: workspace })];
```

Requires bash and a writable workspace. The chosen Environment prepares and executes the
exact published package through its `./runtime` export. The Environment owns isolation and resource grants.
Omit `env` to run in the registering Node application; pass `{ env: workspace }` for an isolated prepared Environment. The factory accepts no configuration options.

Run `npm test --workspace @aexhq/tool-bash` from the repository root.
