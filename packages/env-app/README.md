# @aexhq/env-app

Route callback-hosted Tool invocations to the author's own application.

The model sees ordinary Tools; the implementations never leave the app's process. Declare each
Tool's schema with `appTool(...)` at session create, and register the function with `appTools`
where it lives:

```js
// Session composition: schemas declared, code stays home.
import { appTool } from "@aexhq/brain";
import { app } from "@aexhq/env-app";
import { z } from "zod";

const channel = app({ channelToken: process.env.AEX_CHANNEL_TOKEN });
const invoice = appTool({
  name: "create_invoice",
  description: "Create an invoice in this app.",
  input: z.object({ customer_id: z.string(), amount_cents: z.number().int() }),
}).useIn(channel);
```

```js
// The app's process: hold the channel outward and answer invocations.
import { appTools } from "@aexhq/brain";

const tools = appTools.connect({ url: "wss://…/environments/app/channel", token });
tools.register({ name: "create_invoice", description: "…", input }, async (input) => billing.createInvoice(input));
```

The Environment terminates the app's outbound WebSocket channel, authenticated by the configured
`channelToken`, and answers Brain's invocations from it — each call is one JSON frame carrying the
same `Outcome` envelope as every other Tool hosting, with best-effort cancel down the same
channel. While the app is disconnected, invocations answer with a typed `app_disconnected` error.

A backend that can listen may mount `appTools({ signingKey }).fetchHandler()` instead and receive
HMAC-signed POSTs; both directions register the same way.
