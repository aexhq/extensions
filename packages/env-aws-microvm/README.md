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

## What it executes and declares

The Environment launches `esm` and `shell` programs and declares `fs` and `process`, all
reported on its setup and attach receipts:

- `esm` programs (`brain build` artifacts) are imported once at attach and run in the guest's
  host process, in the workspace, on node's own APIs.
- `shell` programs run as `bash -lc` in the workspace, killed at the call's deadline, with
  captured output capped at the declared `output_bytes_max`.
- `fs` is rooted at the workspace (`/workspace` in the guest; `AEX_WORKSPACE_ROOT` in a local
  test), and every program starts there. `process` declares that programs can start processes.

Enforcement is the guest's, not a wrapper's: the workspace mount, the tool user, and the egress
gateway bound what a program can reach.

The deployed image points `AEX_TOOL_ARTIFACT_DIR` at its installed `*.tool.json` artifacts
(`brain build` output) so attach provisions can be served by content identity; an attach naming an
`esm` identity the host cannot serve fails its receipt.

Every returned Environment reference is session-scoped and can expose provider-specific methods:

```js
await environment.suspend();
```

`suspend()` releases the current AWS incarnation; a later Tool call materializes a fresh one. The
Environment driver rejects lifetimes above its deployment maxima; omitted values use those finite
maxima.
