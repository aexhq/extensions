# @aexhq/tools

Official Tool extensions for Brain.

```js
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { bash, edit, read, write } from "@aexhq/tools";

const workspace = awsMicroVm({ region: "eu-west-2" });
const tools = [
  read().useIn(workspace),
  edit().useIn(workspace),
  write().useIn(workspace),
  bash().useIn(workspace),
];
```

Each factory returns an immutable Tool. `useIn` selects its exact Environment; the Tool's bundled
Node implementation and npm dependencies execute there, never in Brain.
