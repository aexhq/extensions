# @aexhq/env-modal

A finite Modal Sandbox Environment using Brain's public `environment/v1` protocol.
The controller holds provider credentials; commands run as UID/GID 1000 with no inherited
capabilities or privilege escalation. Commands receive one JSON packet on stdin and return
one JSON value on stdout. Brain's offered invocation callback is not passed to commands.

```js
import { modal } from "@aexhq/env-modal";
import { tool } from "@aexhq/brain";
import { z } from "zod";

const workspace = modal({ name: "workspace", url: process.env.ENVIRONMENT_URL,
  token: process.env.ENVIRONMENT_TOKEN, profile: "analysis", lifetimeMs: 300_000 });
const calculate = tool({ name: "calculate", description: "Calculate a result",
  input: z.object({ value: z.number() }), implementation: { type: "modal_command", name: "calculate" } });
const tools = [calculate({ env: workspace })];
```

The operator publishes a Modal image and fixed command catalog before exposing a profile.
Images must contain `/usr/bin/setpriv`, their runtime and data, and a working directory writable
by UID/GID 1000. Use immutable `im-…` IDs. The controller never installs dependencies or accepts
customer image names, arguments, environment variables, secrets, volumes or network rules.

The stdin packet is `{input, configuration, invocation: {sessionId, environment, sequence}}`.
`input` is the model-supplied Tool argument; optional `implementation.configuration` is bound by
the application at session creation (otherwise `null`). The controller supplies invocation identity.
Commands validate both their arguments and configuration. This supports a run-scoped application
callback credential without giving commands a database administrator or provider key. Application
configuration cannot change the image, command, resource size or network profile. It is not stored
in the controller's invocation table; Brain retains the Tool binding and result.

```js
import { createModalEnvironment, serveEnvironment } from "@aexhq/env-modal/server";

const environment = await createModalEnvironment({
  directory: "/var/lib/environment", appName: "application-tools",
  profiles: { analysis: {
    image: process.env.MODAL_IMAGE_ID,
    commands: { calculate: ["python", "/tools/calculate.py"], search: ["python", "/tools/search.py"] },
    cpu: 1, memoryMiB: 1024, maxLifetimeMs: 300_000, region: "us",
  } },
});
const server = await serveEnvironment(environment.handle, { token: process.env.ENVIRONMENT_TOKEN });
```

Create the named Modal App during provisioning. Configure `MODAL_TOKEN_ID` and
`MODAL_TOKEN_SECRET` only on the controller. `createModalClient()` supplies bounded calls with
automatic retries disabled; use it if injecting a real client. The stable Sandbox backend is
required (`MODAL_SANDBOX_V2` must be disabled).

Profiles fix both requested and maximum CPU/memory. Network access is blocked by default;
an explicit `outboundDomains` list can grant selected destinations. No tunnels, private
cross-sandbox networking, cloud mounts or identity tokens are enabled. Region selection can
increase provider charges; see [Modal's region pricing](https://modal.com/docs/guide/region-selection).

## Lifecycle and accounting

`setup` persists immutable configuration and an absolute deadline, without allocating compute.
First use commits a resource name and tags before creating one sandbox. All commands in the
binding share that resource. Different bindings have different writable files. Modal accepts
whole-second timeouts, so remaining lifetime and execution deadlines are rounded down; less
than one second remaining cannot start work.

`cancel`, `detach` and `teardown` terminate the entire sandbox and await provider confirmation,
including other active commands. A stopped, expired or lost binding is never recreated. Treat
its workspace as temporary; persist business results outside the sandbox before ending it.
Tool exit failures and output limits are explicit. Uncertain dispatch or termination remains
`unknown`. Repeated invocation sequences never execute again; the Brain journal owns results.

Run **one controller process per state directory** on retained disk. SQLite commits allocation
and invocation identities before dispatch. `close()` closes the controller, without terminating
cloud resources. Call `reconcile()` periodically and after a restart to observe the original
resources, terminate expired ones and deliver usage. Provider timeouts still bound resources
while the controller is offline. Retain the directory until all resources are confirmed stopped.
Use `recover({sessionId, environment, sandboxId})` for a lost creation response when an operator
has located the original ID. It verifies the original tags and terminates that resource.
A missing running name is not proof that creation never happened.

Hosted products inject `authorize(binding)` and `report(usage)`. Authorization returns an
absolute expiry no later than the caller's requested lifetime, after product admission and a
credit reservation. It is checked again before each command so products can revoke further
dispatch; the expiry must remain unchanged. `configuration.authorization` identifies the grant and never
reaches the command process. Reports contain resource identity and cumulative milliseconds,
including allocated waiting time, capped by the original expiry. A terminal report is emitted
only after provider confirmation, or when allocation was never dispatched. Products deduplicate
these facts, apply their own immutable prices and release unused holds. Reporting failures
retain state for reconciliation. The extension has no account database, wallet or payment rules.

## Verify

`npm test --workspace @aexhq/env-modal` covers durable identities, concurrent first use, uncertain
creation, isolation, admission, cancellation and output limits. `npm run test:integration
--workspace @aexhq/env-modal` requires real Modal credentials and creates three sandboxes with
lifetimes of at most 90 seconds. It verifies filesystem sharing/isolation, controller restart,
network/credential restrictions, non-root execution, descendant cancellation and provider expiry.
All created compute is terminated and checked. The test runner exits after completed hooks
because Modal 0.10.1 leaves its gRPC channels open when `close()` is called.

The protected `modal-integration.yml` workflow must pass at the exact release commit before
publishing. Environment credentials belong in ignored files or deployment secrets.
