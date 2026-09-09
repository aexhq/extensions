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

The package supplies schemas and an opaque official implementation descriptor. The Environment owns runtime preparation
and resource grants; there is no universal dependency declaration.
Brain validates and transports that contract; the selected Environment driver must understand the
descriptor and perform the operation within its own workspace and resource limits. Brain neither
installs Node packages nor compiles the implementation.

The build emits Node 22 bundles and a manifest-digest registry for Environment implementations.
You supply the Environment server and its workspace isolation; this package does not provide a sandbox.
These runtime bundles execute outside the Brain process.

| Tool | Environment prerequisites |
| --- | --- |
| `bash` | Bash and a writable workspace |
| `read`, `ls`, `glob` | A readable workspace |
| `write`, `edit`, `todo` | A writable workspace |
| `grep` | Ripgrep and a readable workspace |

These are runtime prerequisites, not a Brain dependency manifest. Supply them in an image or
prepare them in the Environment's loader before executing a bundle. A Tool reports missing
programs, denied access, and write failures; it does not install packages or retry implicitly.

Place the same factory in several named Environments to authorize each pair. The canonical Tool
definition must match across placements; each pair fixes its own implementation. Configure access through the chosen Environment.

`todo` rejects a competing write to the same workspace with an explicit conflict; it does not
queue or retry writes. Reads return the last committed list. A process interrupted during a write
may leave `.aex/todo.pending`; inspect the list and confirm no writer is active before removing it.
The Agentloop decides whether to report an error to the model and whether to request another call.
