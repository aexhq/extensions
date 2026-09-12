# @aexhq/env-local

A Docker workspace Environment for standalone Brain 0.22. Each named session binding owns one volume. Tools run in fresh containers; files remain across calls and detach. The driver consumes the existing `@aexhq/tools` implementation descriptors and operator-prepared Python projects.

Build a workspace image from the extensions repository:

```sh
npm ci
npm run build --workspace @aexhq/tools
docker build -t my-workspace -f packages/env-local/image/Dockerfile .
```

Run the Environment in a trusted operator process with Docker access:

```js
import { createLocalEnvironment, serveEnvironment } from "@aexhq/env-local/server";

const runtime = await createLocalEnvironment({
  directory: "/var/lib/aex-local",
  profiles: {
    coding: { image: "my-workspace", workspace: "write" },
    reading: { image: "my-workspace", workspace: "read" },
  },
});
const server = await serveEnvironment(runtime.handle, {
  token: process.env.ENVIRONMENT_TOKEN,
  host: "127.0.0.1",
  port: 8090,
});
```

Compose Tools in the application:

```js
import { local } from "@aexhq/env-local";
import { read, write, bash } from "@aexhq/tools";

const workspace = local({ name: "workspace", url: "http://127.0.0.1:8090",
  token: process.env.ENVIRONMENT_TOKEN, profile: "coding" });
const tools = [read({ env: workspace }), write({ env: workspace }), bash({ env: workspace })];
```

The image must already exist. Setup resolves its immutable Docker image ID and records the profile, so changing a tag or operator profile does not change an existing binding. A profile fixes workspace access, memory (default 256 MiB), and process count (default 64). Execution has no network, host mounts, Docker socket or host credentials; its root filesystem is read-only and runs as UID 1000 without capabilities. The operator process and its Docker daemon are privileged deployment components; do not expose either to Tool code.

## Lifecycle and failures

Setup records the binding; the first execution allocates and initializes the volume. Concurrent first calls share that preparation. A failed or interrupted allocation is not silently attempted again. Inspect it and use a new binding. A missing retained volume returns `resource_lost` and is never replaced with an empty one.

The caller's deadline and `cancel` remove the invocation's container, including descendants. The execution returns a failure receipt with code `timeout` or `cancelled`, retaining that cause if cleanup also fails; `details.cleanup_error` describes the cleanup failure. Neither code promises rollback. Missing terminal runner results and lost responses after dispatch remain `unknown`. A known result followed by failed cleanup reports `cleanup_failed` and retains the output in details. Container identities include the binding and journal sequence, so cancellation after a controller restart can find the original container. Detach retains the volume and removes abandoned invocation containers; teardown removes those containers and the volume. Active operations return `busy` from lifecycle cleanup. Teardown can be repeated.

Keep the state directory on retained disk and run one controller per directory. State survives controller process restart; this package does not promise host-loss recovery, backups, cross-controller ownership, persistent shell processes or background callbacks. An interrupted allocation may require operator cleanup. Docker/API errors remain visible and no effect is replayed automatically.

## Python projects

Prepare dependencies in an image using normal packaging. The example uses a locked `pyproject.toml` and `uv.lock`, runs setup before imports at execution time, and then runs without network or installation:

```sh
docker build --build-arg WORKSPACE_IMAGE=my-workspace -t my-python-workspace \
  -f packages/env-local/image/Dockerfile.python .
```

Configure a profile with that image. Declare a normal Brain Tool with `implementation: { type: "python_project", name: "versions" }` and input `{ version: "1.2.3rc1" }`. It returns `{ normalized: "1.2.3rc1", prerelease: true }`. The image's `/opt/aex/python-projects.json` selects the project directory and module; the session cannot select arbitrary paths or modules. A module reads JSON stdin and writes one JSON value to stdout. Filesystem writes go to `/workspace` within the profile's grants.

Images are the preparation boundary: reuse follows image/installation lifetime and there is no runtime installer or universal `needs` manifest. These images run Tools only; Agentloop descriptors are explicitly unsupported.

## Verification

From the repository root, install Bash, ripgrep, Docker and Node 22+, then run `npm run test:prepare`. Run `npm test --workspace @aexhq/env-local`; the tests require real Docker images and never skip missing dependencies. Public-SDK journeys additionally require the pinned Brain fixture described in the root README. Hosted Aex currently rejects customer HTTP Environments.
