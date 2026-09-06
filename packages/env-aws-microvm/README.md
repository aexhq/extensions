# @aexhq/env-aws-microvm

Official AWS MicroVM Environment for Brain.

```js
import { awsMicroVm } from "@aexhq/env-aws-microvm";

const sandbox = awsMicroVm({
  name: "sandbox",
  url: process.env.ENVIRONMENT_URL,
  token: process.env.ENVIRONMENT_TOKEN,
  region: "eu-west-2",
});
```

The application supplies the deployed driver's URL and bearer credential. The SDK sends the
credential separately from the immutable Environment configuration; it is not part of a journaled
implementation descriptor. The factory loads no AWS SDK or provider runtime into Brain.

The external driver, guest supervisor, image builder, and egress gateway live in `runtime/` and
are deployed separately. Setup records configuration and granted needs. Execute accepts an
opaque version 1 `aex_official_tool` descriptor, resolves its bundle from the publisher-built
registry, and checks the Tool's needs against the execution and setup grants. It supports the
official Node.js Tools; arbitrary Python programs and Agentloop Components require a different
provider implementation. Brain does not compile programs or install language packages.

The caller chooses lifecycle milestones: setup at session creation, detach at end, teardown at
delete. Detach retains the workspace; teardown releases only this session's named Environment.
There are no session-level TTL options. Deployed AWS services still impose physical idle and
maximum-duration ceilings. Resource loss is reported; the driver does not recreate a lost
workspace or replay an uncertain Tool effect. A lost submission or result returns `unknown`.
