# @aexhq/tools

Official Tool extensions for Brain.

```js
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { bash, edit, read, write } from "@aexhq/tools";

const workspace = awsMicroVm({ region: "eu-west-2" });
const tools = [
  read().useIn(workspace),
  edit().useIn(workspace),
  write().useIn(workspace),
  bash().useIn(workspace),
];
```

Each factory returns an immutable Tool. `useIn` selects its exact Environment; `brain build`
produces a self-contained ESM artifact that the Environment provisions and executes — never Brain.

Every Tool declares the capabilities it needs, and the declaration is the whole contract: Brain
binds a Tool only to an Environment whose `provides` covers its `requires`, and the run context
exposes typed handles for exactly the declared set. Grant policy (fs root confinement, exec
timeout ceilings) is enforced behind the handles by the Environment's providers; a Tool can only
hit it as an error.

| Tool | requires | capability use |
| --- | --- | --- |
| `bash` | `exec` | `exec.run` in the workspace |
| `read`, `write`, `edit`, `ls` | `fs` | `fs.read` / `fs.write` / `fs.list` |
| `glob` | `fs` | walks `fs.list` with a local matcher |
| `grep` | `exec` | drives ripgrep; gitignore awareness and binary detection are not expressible over the v1 `fs` handle |
| `todo` | none | pure; the list lives in the hosted module |
