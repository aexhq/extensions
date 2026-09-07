# @aexhq/tools

Official placed Tool definitions for Brain.

```js
import { environment } from "@aexhq/brain";
import { bash, edit, read, write } from "@aexhq/tools";

const workspace = environment({ url: () => process.env.ENVIRONMENT_URL })({ name: "workspace" });
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

The build emits Node 22 bundles and a manifest-digest registry for Environment implementations.
You supply the Environment server and its workspace isolation; this package does not provide a sandbox.
These runtime bundles execute outside the Brain process.

| Tool | Needs |
| --- | --- |
| `bash` | `pkg:apt/bash`, `file:///workspace?access=write` |
| `read`, `ls`, `glob` | `file:///workspace` |
| `write`, `edit`, `todo` | `file:///workspace?access=write` |
| `grep` | `pkg:apt/ripgrep`, `file:///workspace` |

Place the same factory in several named Environments to authorize each pair. The canonical Tool
definition must match across placements; each pair fixes its own implementation and needs.
