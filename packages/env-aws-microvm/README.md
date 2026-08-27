# @aexhq/env-aws-microvm

Official AWS Lambda MicroVM Environment for Brain.

The package contains a precompiled Environment component and the external driver, guest supervisor,
image builder, and egress gateway in `runtime/`. Brain sees only the canonical Environment lifecycle
and an opaque `aws-microvm` driver binding; no AWS SDK enters the Brain kernel.

```js
import { awsMicroVm } from "@aexhq/env-aws-microvm";

const environment = awsMicroVm({
  region: "eu-west-2",
  idleSeconds: 60,
  maximumSeconds: 3_600,
});
```

Every returned Environment reference is session-scoped and can expose provider-specific methods:

```js
await environment.suspend();
```

`suspend()` releases the current AWS incarnation; a later Tool call materializes a fresh one. The
Environment driver rejects lifetimes above its deployment maxima; omitted values use those finite
maxima.
