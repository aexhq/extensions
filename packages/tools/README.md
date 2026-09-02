# @aexhq/tools

Official Tool extensions for Brain.

```js
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { bash, edit, read, write } from "@aexhq/tools";

const workspace = awsMicroVm({ region: "eu-west-2" });
const tools = [
  read({ env: workspace }),
  edit({ env: workspace }),
  write({ env: workspace }),
  bash({ env: workspace }),
];
```

Each factory returns an immutable Tool placed in the Environment named by `env`. `brain build`
produces one artifact per tool that the Environment launches, never Brain.

Every Tool is a program plus a declaration of the resources it operates on, and the declaration
is the whole contract. Brain binds a Tool only to an Environment that launches its program kind
and declares every resource it needs. Inside, a Tool is plain code on the platform it runs on:
`node:fs`, `child_process`, `fetch`. Nothing is wrapped, and the working directory is the
Environment's workspace root.

| Tool | Program | Needs | How |
| --- | --- | --- | --- |
| `bash` | `shell` | `process` | the command is the script; the Environment runs it in the workspace |
| `read`, `write`, `edit`, `ls` | `esm` | `fs` | `node:fs/promises` on paths relative to the workspace |
| `glob` | `esm` | `fs` | walks `readdir` with a local matcher |
| `grep` | `esm` | `process` | drives ripgrep through `child_process`; gitignore awareness and binary detection come from the binary the image ships |
| `todo` | `esm` | none | pure; the list lives in the hosted module |
