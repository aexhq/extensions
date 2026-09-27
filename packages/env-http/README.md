# Application tools over HTTP

Let Brain call short functions in your existing API. The agent runs in Brain; each function
runs in an ordinary application request, including a serverless request. The submitting
process can exit while the agent thinks. Your application still hosts its business code.

Install `@aexhq/env-http`, `@aexhq/brain` and `zod`. Define tools with Brain's `tool()` API,
then mount `createToolHandler({ tools, authorize })` at one POST route in your existing API.
The handler accepts a standard `Request` and returns a `Response`; Hono can pass `c.req.raw`.
With Aex, declare `aex.environments.application({ name, endpoint, credential, timeoutMs })`
and place ordinary Tools with `{ env }` in `aex.sessions.create()`. Aex prepares their
remote descriptors and admits the endpoint during creation. See the
[Application guide](https://github.com/aexhq/aex/blob/main/docs/http-tools.md).

The executable [records example](examples/records.mjs) implements reads, proposals, reviewed
saves and report jobs. Its [test](test/records.test.mjs) opens a fresh handler for each request,
rechecks membership, rejects another tenant's records, and proves atomic save receipts.
SQLite is its standalone example store; a serverless deployment uses the application's
existing durable database. The application creates the session-to-user/company mapping
before submitting a turn. Model arguments never supply that identity.

`authorize(request, invocation)` must verify the bridge credential, resolve the session's
stored owner and check current access. Tools receive `sessionId`, `sequence`, `deadline`
and `signal`. Application Tools use `await ctx.finish(value)` and `ctx.emitResult(value)`
with Brain's durable acknowledgment. Their schema, configured options and optional
model-facing content retain the ordinary Tool contract. Use the same configured Tools
in the session and handler. Model/event services and Environment control are unavailable
inside this bounded handler. Keep its timeout below the application's HTTP request budget.
Only the bridge holds Brain callback credentials; the handler receives an invocation-scoped
completion capability. A handler's `{status: "cancelled"}` is
successful business data, not a cancelled Brain invocation.

The registry supplies the model schema and runtime validation from the same Tool declarations.
Input and output validation await Zod. A changed advertised contract fails before business
code. Retain compatible handlers or create a new session for breaking changes.

For self-hosted Brain, run `createHttpEnvironment({ authorize })` with `serveEnvironment()`.
Use `application({ name, url, endpoint, credential, timeoutMs })` and
`applicationTool(placedTool, { env })` with a self-hosted authorizer. It resolves an approved
endpoint, credential, tool catalogue, `timeoutMs` and public `callbackUrl` for each
session/environment. Pass the returned `callback` to `serveEnvironment` to enable the
invocation relay. The host owns admission and authentication.
The default transport only connects to public HTTPS addresses on port 443, checks the
actual socket's DNS answers, rejects private/transition addresses and never follows redirects.
Deploy it with the host's egress restrictions and keep its Environment token private.

The bridge sends once and explicitly finishes a successful result. Lost or malformed responses
remain unknown; it does not repeat a business operation. Cancellation aborts HTTP where possible
and does not undo a committed mutation. App-owned operation keys and save receipts resolve
business uncertainty. A submitted report job completes independently: call `get_report` in
the same or a later turn. Ending a turn does not schedule another status check.

## Compatibility and Environment control

Existing `http({ name, url, binding })` and `httpTool(...)` placements keep the v1
return-JSON contract. `createToolHandler` accepts both v1 and Application v2 calls.
Application Tools use `ctx.finish`; v1 handlers continue to return JSON directly.


The provider-defined `inspect` method returns authorized Tool names and the binding timeout;
it does not contact the application or expose its credential. Grant it explicitly through
`environments: [{ environment: "app", permissions: ["read", "call"], methods: ["inspect"] }]`.
Select automatic or manual lifecycle independently when creating the Brain session.
