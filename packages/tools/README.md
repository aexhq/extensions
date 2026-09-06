# @aexhq/tools

Official placed Tool definitions for Brain.

```js
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { bash, edit, read, write } from "@aexhq/tools";

const workspace = awsMicroVm({ name: "sandbox", url: process.env.ENVIRONMENT_URL, token: process.env.ENVIRONMENT_TOKEN, region: "eu-west-2" });
const tools = [
  read({ env: workspace }),
  edit({ env: workspace }),
  write({ env: workspace }),
  bash({ env: workspace }),
];
```

Each factory requires one placement object containing `env` and any Tool-specific options. It
returns an immutable Tool binding. There is no implicit Environment and no `.useIn` step.

The package supplies schemas, resource needs, and an opaque official implementation descriptor.
Brain validates and transports that contract; the selected Environment driver must understand the
descriptor and perform the operation within its own workspace and resource limits. Brain neither
installs Node packages nor compiles the implementation.

This package's build also emits the publisher-owned Node 22 bundles and a manifest-digest registry
used by the AWS MicroVM driver. Those deployment artifacts are not a Brain SDK compiler output and
are never loaded into the Brain process.

| Tool | Needs |
| --- | --- |
| `bash` | `pkg:apt/bash`, `file:///workspace?access=write` |
| `read`, `ls`, `glob` | `file:///workspace` |
| `write`, `edit`, `todo` | `file:///workspace?access=write` |
| `grep` | `pkg:apt/ripgrep`, `file:///workspace` |

Place the same factory in several named Environments to authorize each pair. The canonical Tool
definition must match across placements; each pair fixes its own implementation and needs.
