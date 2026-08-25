# @aexhq/tools

Official Tool components for Brain. Nothing is granted by default; applications select only the
tools a session needs.

```js
import { bash, edit, read, write } from "@aexhq/tools";

const tools = [read(), edit(), write(), bash()];
```

Every factory returns an immutable Tool component declaration. The eight tools share one
precompiled dispatcher component, but each carries its own JSON Schema contract and opaque Node 22
implementation bundle. The dispatcher has only the Environment context grant and passes the bundle
to the Environment selected by the session. Brain neither evaluates the tool module nor requires
the Environment to be a particular operating system, provider or isolation product.
