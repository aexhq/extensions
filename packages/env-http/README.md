# Application tools over HTTP

Let Brain call short functions in your existing API. The agent runs in Brain; each function
runs in an ordinary application request, including a serverless request. The submitting
process can exit while the agent thinks. Your application still hosts its business code.

Install `@aexhq/env-http`, `@aexhq/brain` and `zod`. Define tools with Brain's `tool()` API,
then mount `createToolHandler({ tools, authorize })` at one POST route in your existing API.
The handler accepts a standard `Request` and returns a `Response`; Hono can pass `c.req.raw`.
Use `httpTool(placedTool, { env })` to place each tool in an `http({ name, url, binding })`
Environment. With Aex, select its catalog driver URL and an account-approved binding.

The executable [records example](examples/records.mjs) implements reads, proposals, reviewed
saves and report jobs. Its [test](test/records.test.mjs) opens a fresh handler for each request,
rechecks membership, rejects another tenant's records, and proves atomic save receipts.
SQLite is its standalone example store; a serverless deployment uses the application's
existing durable database. The application creates the session-to-user/company mapping
before submitting a turn. Model arguments never supply that identity.

`authorize(request, invocation)` must verify the bridge credential, resolve the session's
stored owner and check current access. Tools receive `sessionId`, `sequence`, `deadline`
and `signal`; they return JSON. There are no background callbacks or model/event services
inside this bounded handler. Keep its timeout below the application's HTTP request budget.
Only the bridge holds Brain callback credentials. A handler's `{status: "cancelled"}` is
successful business data, not a cancelled Brain invocation.

The registry supplies the model schema and runtime validation from the same Tool declarations.
Input and output validation await Zod. A changed advertised contract fails before business
code; retain compatible handlers or create a new binding and session for breaking changes.

For self-hosted Brain, run `createHttpEnvironment({ authorize })` with `serveEnvironment()`.
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
