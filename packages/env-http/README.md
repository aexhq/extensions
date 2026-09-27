# Application tools over HTTP

Let Brain call short functions in your existing API. The agent runs in Brain; each function
runs in an ordinary application request, including a serverless request. The submitting
process can exit while the agent thinks. Your application still hosts its business code.

You need Node.js 22.13 or newer, an existing application API and an HTTP Environment bridge
that stays running independently of each request. Install the compatible package set:

```sh
npm install @aexhq/env-http@0.2.3 @aexhq/brain@0.34.1 zod@4.4.3
```

## Add an application handler

This TypeScript module composes your existing authorization and order lookup functions.
`authorizeSession` must authenticate the bridge credential, load the stored session owner and
check their current access. `readOrder` uses that session's ownership to scope the lookup;
model arguments supply only an order ID.

```ts
import { tool } from "@aexhq/brain";
import { createToolHandler } from "@aexhq/env-http/handler";
import { z } from "zod";

export function createOrderApi(options: {
  authorizeSession(request: Request, sessionId: string): Promise<void>;
  readOrder(sessionId: string, id: string): Promise<{ id: string; status: string }>;
}) {
  const lookupOrder = tool({
    name: "lookup_order",
    description: "Look up an order by id.",
    input: z.object({ id: z.string() }),
    output: z.object({ id: z.string(), status: z.string() }),
    run: ({ id }, ctx) => options.readOrder(ctx.sessionId, id),
  });
  const tools = [lookupOrder()];
  const handler = createToolHandler({
    tools,
    authorize: (request, call) => options.authorizeSession(request, call.sessionId),
  });
  return { tools, handler };
}
```

Mount the returned `handler` at a POST route. It accepts a standard `Request` and returns a
`Response`; Hono can pass `c.req.raw`. Return plain JSON from an HTTP tool. When moving a tool
from the Brain quickstart, replace `ctx.finish(value)` with `value`: the bridge records completion.
`ctx.finish`, `ctx.emit`, `ctx.emitResult`, `ctx.model` and background callbacks are unavailable here.

## Place the tools in a session

After calling `createOrderApi` with your application functions, use its returned `orderApi.tools`
when creating the session. The URL below is the running bridge, not your application's POST route.
Its operator maps `orders-v1` to your HTTPS route, credential, tool catalogue and request timeout.

```ts
import { http, httpTool } from "@aexhq/env-http";

const app = http({
  name: "app",
  url: "https://bridge.example.com",
  binding: "orders-v1",
  token: bridgeToken,
});
const tools = orderApi.tools.map(tool => httpTool(tool, { env: app }));
```

Use the bridge token supplied by its operator. With Aex, select its catalog driver URL and an
account-approved binding instead. Pass `tools` with your model and loop to
[`brain.sessions.create()`](https://aex.dev/brain/docs/quickstart). A turn submitted with
`session.submit()` can continue after that client closes because tool requests reach the
independently running bridge and your API.

The complete [records example](examples/records.mjs) implements authorization, reads, proposals, reviewed
saves and report jobs. Its [test](test/records.test.mjs) opens a fresh handler for each request,
rechecks membership, rejects another tenant's records, and proves atomic save receipts.
SQLite is its standalone example store; a serverless deployment uses the application's
existing durable database. The application creates the session-to-user/company mapping
before submitting a turn. Model arguments never supply that identity.

Tools receive `sessionId`, `sequence`, `deadline` and `signal`. Keep the binding timeout below
the application's HTTP request budget.
Only the bridge holds Brain callback credentials. A handler's `{status: "cancelled"}` is
successful business data, not a cancelled Brain invocation.

The registry supplies the model schema and runtime validation from the same Tool declarations.
Input and output validation await Zod. A changed advertised contract fails before business
code; retain compatible handlers or create a new binding and session for breaking changes.

## Deploy the bridge

For self-hosted Brain, import `createHttpEnvironment` and `serveEnvironment` from
`@aexhq/env-http/server`, then run `createHttpEnvironment({ authorize })` with `serveEnvironment()`.
The injected authorizer resolves an approved endpoint, credential, tool catalogue and
`timeoutMs` for the authenticated session/environment, on setup and every invocation.
The default transport only connects to public HTTPS addresses on port 443, checks the
actual socket's DNS answers, rejects private/transition addresses and never follows redirects.
Deploy it with the host's egress restrictions and keep its Environment token private.

The bridge sends once and explicitly finishes a successful result. Lost or malformed responses
remain unknown; it does not repeat a business operation. Cancellation aborts HTTP where possible
and does not undo a committed mutation. App-owned operation keys and save receipts resolve
business uncertainty. A submitted report job completes independently: call `get_report` in
the same or a later turn. Ending a turn does not schedule another status check.

## Environment control

The provider-defined `inspect` method returns authorized Tool names and the binding timeout;
it does not contact the application or expose its credential. Grant it explicitly through
`environments: [{ environment: "app", permissions: ["read", "call"], methods: ["inspect"] }]`.
Select automatic or manual lifecycle independently when creating the Brain session.
