# @aexhq/env-aws-microvm

Official AWS Lambda MicroVM Environment for Brain.

The package contains a precompiled Environment component and the external driver, guest supervisor,
image builder, and egress gateway in `runtime/`. Brain sees only the canonical Environment lifecycle
and an opaque `aws-microvm` driver binding; no AWS SDK enters the Brain kernel.

```js
import { awsMicrovm } from "@aexhq/env-aws-microvm";

const environment = awsMicrovm({
  region: "eu-west-2",
  idleSeconds: 60,
  maximumSeconds: 3_600,
});
```

Both lifetimes are finite. The Environment driver rejects values above its deployment maxima;
omitted values use those finite maxima.
