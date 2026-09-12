# @aexhq/env-browser

A Playwright Environment and five Tools for Brain 0.22: `browser_navigate`, `browser_inspect`, `browser_click`, `browser_fill` and `browser_screenshot`.

The operator supplies a launcher for each allowed profile. Every binding receives a dedicated browser instance, context and page. For a local development deployment:

```js
import { chromium } from "playwright";
import { createBrowserEnvironment, serveEnvironment } from "@aexhq/env-browser/server";

const runtime = createBrowserEnvironment({
  profiles: { web: () => chromium.launch({ chromiumSandbox: true }) },
});
const server = await serveEnvironment(runtime.handle, {
  token: process.env.ENVIRONMENT_TOKEN, host: "127.0.0.1", port: 8091,
});
```

Install Chromium and its OS dependencies with `npx playwright install --with-deps chromium`. Run Chromium as a non-root user with its sandbox available. A production launcher must supply an isolated browser process, enforce its network authority at the deployment boundary, and supervise cleanup after controller loss. Browser contexts separate cookies and storage; they are not a hostile-tenant execution sandbox. The local launcher above grants the browser the operator process's network reachability. Do not use it as a public multi-tenant browsing service.

Compose it through the public SDK:

```js
import { browser, browserTools } from "@aexhq/env-browser";

const web = browser({ name: "web", url: "http://127.0.0.1:8091",
  token: process.env.ENVIRONMENT_TOKEN, profile: "web" });
const tools = browserTools({ env: web });
```

Individual factories are also exported as `navigate`, `inspect`, `click`, `fill` and `screenshot`. They require `{ env }`. Click and fill use Playwright locator selectors; they require exactly one matching element. Navigation accepts HTTP(S). Inspection returns URL, title, an accessibility snapshot and an explicit truncation flag above 64 KiB. Screenshots capture the fixed 1280×720 viewport as PNG.

## State and cancellation

Allocation is lazy. Calls on one binding serialize in arrival order; calls on different bindings can run concurrently. Put dependent actions such as fill then submit in separate dispatch batches: concurrent HTTP arrival order is not model source order. Additional windows are closed, service workers are blocked and downloads are disabled in this single-page profile.

The context and page persist across turns and detach. Teardown closes the browser and can be repeated. A deadline or Playwright timeout returns a failure receipt with code `timeout`; explicit cancellation returns `cancelled`. Both close an active browser and retain their cause if cleanup fails, with `details.cleanup_error` describing the cleanup failure. These codes do not promise rollback of clicks or other effects. Browser loss during an action without a reliable result remains `unknown`. Subsequent calls report resource loss and never create a replacement browser. Cancelling a call that is still queued does not cancel a preceding call.

Browser crash/closure is reported on the next operation. The execution token is never retained for background notifications. Call `runtime.close()` during operator shutdown. A restarted controller cannot recover a prior browser identity; operations, including teardown, report `resource_lost`. Its deployment supervisor must reclaim any surviving process. Durable remote-browser reattachment is outside this reference profile.

## Image presentation

Use Pi or Codex **5.1.0 or later**. Screenshot output uses the official `aex_tool_output` version 1 presentation convention. These loops map its image into Brain's existing Tool-result `media`, while the journal retains the raw output. Older loops treat the result as ordinary JSON and do not provide a visual workflow. A custom loop can perform the same mapping; this package introduces no new Brain protocol.

## Verification

Run `npm test --workspace @aexhq/env-browser` after installing Chromium and its dependencies. Tests launch real sandboxed Chromium against a local fixture and cover state, session separation, arrival serialization, cancellation, loss, screenshots and cleanup. Public-SDK journeys exercise images through both compiled official loops. Hosted Aex currently rejects customer HTTP Environments.
