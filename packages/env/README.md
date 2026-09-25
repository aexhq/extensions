# env

Give a Brain model controlled access to its environments. This is an ordinary Tool over
`ctx.environments`, with the same interface available to any third-party Tool.

```ts
import { env } from "@aexhq/env";

const debug = env({
  environments: [{ environment: "workspace", permissions: ["read", "call"], methods: ["inspect"] }],
});
// Include debug in your session's tools; lifecycle defaults to automatic.
```

It defaults to your application's host. Place it in a prepared Node Environment to run it
remotely. Keep that placement independent of any environment it may delete or replace.

The input operations are `list`, `get`, `create`, `update`, `setup`, `delete` and `call`.
Use the current `{ name, sequence }` reference returned by `list` or `get`. `get` reads
recorded state; `call` invokes a method declared by the Env. There is no universal inspect
or restart method. Resources such as tabs and jobs use the Env's own methods and schemas.

Creation requires an application-authorized template and a `create` grant. Setup defaults to
automatic; override it with `environment.lifecycle`. Adding this Tool never changes it. See Brain's
[environment control guide](https://aex.dev/brain/docs/guides/environment-control).
