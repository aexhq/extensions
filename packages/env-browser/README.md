# @aexhq/env-browser

A Playwright Environment and five Tools for Brain: `browser_navigate`, `browser_inspect`, `browser_click`, `browser_fill` and `browser_screenshot`.

The operator supplies a launcher for each allowed profile. Every binding receives a dedicated browser instance, context and page. For a local development deployment:

```js
import { chromium } from "playwright";
import { createBrowserEnvironment, serveEnvironment } from "@aexhq/env-browser/server";

const runtime = createBrowserEnvironment({
  profiles: { web: () => chromium.launch({ chromiumSandbox: true }) },
  publishMedia: publishScreenshot,
});
const server = await serveEnvironment(runtime.handle, {
  token: process.env.ENVIRONMENT_TOKEN, host: "127.0.0.1", port: 8091,
});
```

Install Chromium and its OS dependencies with `npx playwright install --with-deps chromium`. Run Chromium as a non-root user with its sandbox available. A production launcher must supply an isolated browser process, enforce its network authority at the deployment boundary, and supervise cleanup after controller loss. Browser contexts separate cookies and storage; they are not a hostile-tenant execution sandbox. The local launcher above grants the browser the operator process's network reachability. Do not use it as a public multi-tenant browsing service.

Ubuntu's AppArmor policy may require a [user-namespace profile for downloaded Chromium](https://chromium.googlesource.com/chromium/src/+/main/docs/security/apparmor-userns-restrictions.md). CI installs a profile for its Playwright headless-shell path and keeps Chromium sandboxing enabled.

Compose it through the public SDK:

```js
import { browser, browserTools } from "@aexhq/env-browser";

const web = browser({ name: "web", url: "http://127.0.0.1:8091",
  token: process.env.ENVIRONMENT_TOKEN, profile: "web" });
const tools = browserTools({ env: web });
```

Individual factories are also exported as `navigate`, `inspect`, `click`, `fill` and `screenshot`. They require `{ env }`. Click and fill use Playwright locator selectors; they require exactly one matching element. Navigation accepts HTTP(S). Inspection returns URL, title, an accessibility snapshot and an explicit truncation flag above 64 KiB. Screenshots capture the fixed 1280Ã—720 viewport as PNG.

## State and cancellation

Allocation is lazy. Calls on one binding serialize in arrival order; calls on different bindings can run concurrently. Put dependent actions such as fill then submit in separate dispatch batches: concurrent HTTP arrival order is not model source order. Additional windows are closed, service workers are blocked and downloads are disabled in this single-page profile.

The context and page persist across turns and detach. Teardown closes the browser and can be repeated. A deadline or Playwright timeout returns a failure receipt with code `timeout`; explicit cancellation returns `cancelled`. Both close an active browser and retain their cause if cleanup fails, with `details.cleanup_error` describing the cleanup failure. These codes do not promise rollback of clicks or other effects. Browser loss during an action without a reliable result remains `unknown`. Subsequent calls report resource loss and never create a replacement browser. Cancelling a call that is still queued does not cancel a preceding call.

Browser crash/closure is reported on the next operation. The execution token is never retained for background notifications. Call `runtime.close()` during operator shutdown. A restarted controller cannot recover a prior browser identity; operations, including teardown, report `resource_lost`. Its deployment supervisor must reclaim any surviving process. Durable remote-browser reattachment is outside this reference profile.

## Image presentation

Supply `publishMedia({ bytes, mediaType, index }, { sessionId, sequence, signal })` when creating
the Environment. It must store the bytes and return an HTTPS URL reachable by the model provider.
The screenshot call fails explicitly if publication is unavailable or fails. The callback runs
before output is returned; PNG bytes never enter the retained Tool result.

The host owns storage, expiry and publication credentials. It may use Aex's attachment upload API
or another store. Keep credentials in the Environment operator process and use the supplied signal
to cancel publication. Use session ID, invocation sequence and media index to scope upload
idempotency. Reattaching or observing an uncertain call does not take another screenshot.

Use the matching URL-media releases of Pi or Codex. Screenshot output uses `aex_tool_output`
version 1; these loops map its URL into Brain Tool-result `media`. A custom loop can perform the
same mapping. URLs must stay available for later turns that need the image.

## Verification

Run `npm test --workspace @aexhq/env-browser` after installing Chromium and its dependencies. Tests launch real sandboxed Chromium against a local fixture and cover state, session separation, arrival serialization, cancellation, loss, screenshots and cleanup. Public-SDK journeys exercise images through both compiled official loops. Hosted Aex currently rejects customer HTTP Environments.
