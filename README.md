# extensions

Official agent loop, Tool, and Environment extensions for Aex. Every package uses the public
`@aexhq/brain` authoring API and the same `brain build` pipeline; official extensions have no
privileged runtime path.

| package | role |
| --- | --- |
| `@aexhq/agentloop-pi` | Pi-style agent loop with parallel Tool calls |
| `@aexhq/agentloop-codex` | Codex-style agent loop with sequential Tool calls |
| `@aexhq/tools` | Model-visible Tool definitions with provisioned ESM implementations |
| `@aexhq/env-aws-microvm` | AWS MicroVM Environment and provider runtime |

All three roles have the same authoring shape:

```ts
import { agentloop } from "@aexhq/brain";

export const support = agentloop((author) => {
  author.turn(async (turn) => {
    turn.transcript.push({ role: "user", content: [{ type: "text", text: turn.input.message }] });
    const { message } = await turn.model({ messages: turn.transcript });
    turn.transcript.push(message);
    await turn.reply(message.content.map((block) => block.text ?? "").join(""));
    return turn.done();
  });
});
```

```sh
brain build
```

An agent loop drives one whole turn: it calls the model and dispatches tools through the
`turn` object, and Brain journals every call before it happens.

Tools and Environments meet through the execution contract. A Tool is a program (an `esm`
module, a `shell` script, or an `http` request) plus the resources it operates on (`fs`,
`process`, `net`, `dom`, `secrets`). An Environment declares the resources a program finds there
and registers an executor for each program kind it launches (`execute.esm()`,
`execute.shell(...)`, `execute.http(...)`). Brain checks the tool's program kind and `needs`
against the environment's declaration at session create and never sees inside either half.
Inside, a Tool is plain code on the platform it runs on:

```ts
export const test = tool({
  description: "Run the test suite.",
  input, output,
  needs: ["process", "fs"],
}, (author) => {
  author.run(async (input, context) => runTests(input, { signal: context.signal }));
});

export const bash = tool.shell({
  description: "Run a shell command.",
  input, output,
  needs: ["process"],
  script: "$command",
});
```

Applications place a Tool explicitly with the factory's `env` option —
`bash({ env: environment })` — and can call extension-owned methods on the same Environment
object. A tool whose function lives in an application process needs no environment at all:
declare it with `tool(...)` (with `execute` to run beside the session's creator, without one
to be served by whatever process joins the session with its share key) — Brain routes those
invocations itself.
