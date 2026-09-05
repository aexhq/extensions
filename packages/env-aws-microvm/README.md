# @aexhq/env-aws-microvm

Official AWS MicroVM Environment configuration for Brain.

```js
import { awsMicroVm } from "@aexhq/env-aws-microvm";

const environment = awsMicroVm({
  region: "eu-west-2",
  idleSeconds: 60,
  maximumSeconds: 3_600,
});
```

The public factory returns an immutable Environment descriptor for the `aws-microvm` driver. It
contains deployment configuration only: no AWS SDK or provider runtime is loaded into the Brain
process, and the Environment exposes no provider-specific lifecycle methods.

The external driver, guest supervisor, image builder, and egress gateway live in `runtime/` and
must be deployed separately. That driver owns execution, workspaces, quotas, cancellation, and
resource enforcement, and must explicitly support any opaque Tool implementation it accepts.
Brain validates and transports the Environment and Tool contracts but does not compile programs or
install language packages.

The supplied driver accepts version 1 `aex_official_tool` descriptors, binds them to the complete
Tool manifests received during `attach`, and checks the manifests against its publisher-built
runtime registry before invoking a bundle.

`idleSeconds` cannot exceed `maximumSeconds`. The deployed driver may impose stricter finite
limits and rejects unsupported configuration rather than silently weakening it.
