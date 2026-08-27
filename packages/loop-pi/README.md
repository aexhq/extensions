# @aexhq/brain-pi

The official Pi-style Brain extension. It owns conversation context and dispatches independent Tool
calls in parallel while Brain owns durable execution, provider calls, and journaling.

```js
import { pi } from "@aexhq/brain-pi";

const extension = pi();
```

The built Brain artifact is admitted automatically when a session is created.
