# @aexhq/env-app

Route Tool operations to a registered customer application through the canonical Environment
lifecycle.

```js
import { app } from "@aexhq/env-app";

const environment = app({ id: "billing-api" });
```

The factory returns a precompiled Environment declaration. `customer` is an opaque external driver
binding, not a kernel profile; it follows the same lifecycle as every other Environment.
