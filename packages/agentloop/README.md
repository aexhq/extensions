# @aexhq/agentloop

Author Agentloop components for Brain sessions. An Agentloop is capability-pure policy code;
Brain executes and journals its effects through the canonical `aex:agentloop@1.0.0` world.

```js
import { defineAgentloop } from "@aexhq/agentloop";

export const { activate } = defineAgentloop({
  async onMessage(ctx, message) {
    const round = await ctx.model.stream({
      system: ctx.config.instructions,
      messages: [{ role: "user", content: message.content }],
    });
    const calls = round.content.filter((block) => block.type === "tool_call");
    if (calls.length > 0) await ctx.tools.dispatch(calls);
    await ctx.kv.set({ last_stop_reason: round.stop_reason });
  },
});
```

The context exposes the sealed session, immutable package configuration, cancellation, model,
tools, journal, key/value state, and turn terminal. A fresh component instance receives durable
hydration through `onSessionStart`; resident memory is only a cache.

Build the component before publishing it. The compiler is explicit and replaceable:

```js
import { componentize } from "@bytecodealliance/componentize-js";
import { buildAgentloopComponent } from "@aexhq/agentloop/build";

const built = await buildAgentloopComponent({ entry: "./my-loop.mjs" }, componentize);
// Publish built.component. Brain verifies its SHA-256 and canonical WIT identity; it does not
// compile JavaScript or select a compiler.
```

`buildLoopBundle` is also exported for compiler integrations. Its source digest is build
provenance, not runtime identity.
