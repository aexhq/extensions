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

## Capabilities

The Environment provides `exec` and `fs`, reported on its setup and attach receipts, and hosts
provisioned ESM Tool artifacts (`host.esm`):

- `exec` runs `bash -lc` on the VM, with the attachment's exec grant enforced behind the handle —
  the granted `timeout_ms_max` clamps every requested timeout (enforced by kill) and
  `output_bytes_max` caps captured output.
- `fs` is rooted at the attachment's granted `fs.root`: every path is confined with `clamp.path`,
  writes create parent directories, and an attachment without an fs grant is denied by default.

The deployed image points `AEX_TOOL_ARTIFACT_DIR` at its installed `*.tool.json` artifacts
(`brain build` output) so attach provisions can be served by content identity; an attach naming an
identity the host cannot serve fails its receipt.

Every returned Environment reference is session-scoped and can expose provider-specific methods:

```js
await environment.suspend();
```

`suspend()` releases the current AWS incarnation; a later Tool call materializes a fresh one. The
Environment driver rejects lifetimes above its deployment maxima; omitted values use those finite
maxima.
