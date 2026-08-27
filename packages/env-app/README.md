# @aexhq/env-app

Route Tool operations to a registered customer application through the canonical Environment
lifecycle.

```js
import { app } from "@aexhq/env-app";

const client = app();
```

The factory returns an ordinary Environment reference. The `./provider` export supplies the HTTP
adapter for application-owned handlers; it follows the same setup, attachment, execution, call,
cancellation, detachment, and teardown contract as every other Environment.
