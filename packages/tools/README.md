# @aexhq/tools

Official Tool components for Brain. Nothing is granted by default; applications select only the
tools a session needs.

```js
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { bash, edit, read, write } from "@aexhq/tools";

const workspace = awsMicroVm({ region: "eu-west-2" });
const tools = [
  read().runIn(workspace),
  edit().runIn(workspace),
  write().runIn(workspace),
  bash().runIn(workspace),
];
```

Every factory returns an immutable model-visible Tool definition. `runIn` selects the Environment
that executes it; Brain stores only the remote binding and never evaluates the Tool implementation.
The package separately exports `handlers` for Environment implementations that run the official
Node 22 handlers.
