# @aexhq/tools

Official Tool components for Brain. Nothing is granted by default; applications select only the
tools a session needs.

```js
import { bash, edit, read, task, write } from "@aexhq/tools";

const tools = [read(), edit(), write(), bash(), task()];
```

Every factory returns an immutable Tool component declaration. The eight tools share one
precompiled dispatcher component, but each carries its own JSON Schema contract and opaque Node 22
implementation bundle. The dispatcher has only the Environment context grant; the sealed
configuration names its bundle by digest and the bundle bytes travel once as a content-addressed
create-time artifact layer, so Brain hands the Environment exactly the code the session admitted.
Brain neither evaluates the tool module nor requires the Environment to be a particular operating
system, provider or isolation product.

`subagents()` (also exported as `task()`) is not an extension at all: it declares Brain's builtin
`brain.subagents` capability, which a customer turns on by selecting it. Spawning reaches parent
and child session data, so it resolves inside Brain and never crosses the component host. It can
create, message, inspect, wait for, list, interrupt, or end durable direct child sessions, and it
ships no component and needs no Environment binding.
