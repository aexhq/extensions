# The environment image

`Dockerfile` builds the sandbox the agent's tools run inside: the `environment-guest` binary plus a
curated toolchain (git, Python, Node, ripgrep, build-essential, common archivers). ARM64.

The binary uses the aarch64 GNU target to match the Ubuntu base. The resulting image is a thin
runtime layer that rebuilds quickly when only code changes:

```
# on an aarch64 Linux host (or cross with aarch64-linux-gnu-gcc)
cargo build -p environment-guest --release --target aarch64-unknown-linux-gnu
docker build -f image/Dockerfile \
  --build-arg BIN=target/aarch64-unknown-linux-gnu/release/environment-guest \
  -t aex-environment:dev .
docker run --rm --sysctl net.ipv4.ip_unprivileged_port_start=8081 \
  -p 8080:8080 aex-environment:dev
```

The hosted MVP has one physical shape: 0.5 baseline vCPU and exactly 1,024 MiB of provider
memory. The image publisher deliberately has no memory-size argument. Publication and the AWS
runtime adapter consume the same `MVP_TARGET_MEMORY_MIB` constant, so capacity admission cannot
charge one shape while an operator publishes another.

Installer output is redirected into `/workspace/.environment/**` (CARGO_HOME, GOPATH, npm prefix, pip/uv
caches, pipx) so it remains available for the life of the sandbox generation. Nothing in the live
workspace is checkpointed or restored implicitly; system-wide installs and workspace changes are
both lost when that generation is gone unless the caller explicitly copies data to session
storage.

The Environment listens on `:8080` for its trusted external supervisor connection. Every protected HTTP
or WebSocket request requires a random generation bearer delivered only in the sealed provider run
payload and retained with the durable target route; it is never projected through Brain, logs,
argv, or Tool environments. Both UID 1000 additional shells and dynamically allocated managed
binding UIDs may share the guest network namespace, but receive only `401` from the live control
endpoint without that bearer. Root boot also raises `ip_unprivileged_port_start` to 8081. A tiny
root-owned, group-restricted launcher binds port 8080 first, drops to the supervisor identity, and
passes only that socket descriptor to `environment-guest`; a background Tool therefore cannot bind or
impersonate the port after a supervisor crash. CI proves both Tool identity classes cannot
authenticate to a live supervisor or bind the released control port.

The build removes every package-owned setuid/setgid bit and inherited file capability. The
root-owned listener launcher carries only `CAP_KILL`, `CAP_SETGID`, and `CAP_SETUID` across its drop
to the supervisor identity and into `environment-guest`; this does not depend on the provider honoring
filesystem capabilities. The supervisor needs the UID/GID pair to spawn Tool children and
`CAP_KILL` to enforce their deadlines after the UID split; Linux does not give parents an implicit
cross-UID signal right. Children clear every capability set and set
`no_new_privs`. Each immutable managed binding receives a distinct deterministic UID
from a bounded 4,096-entry per-generation registry; collisions and exhaustion fail permanently
instead of aliasing secret subsets. Parallel calls of the same binding deliberately share its UID
because they have the same immutable environment-name subset. Additional-sandbox shells remain UID
1000. All Tool identities share GID 1000 and a group-friendly `0002` umask, so ordinary workspace
outputs remain collaborative. A Tool that explicitly creates a `0600` file makes it binding-private
and therefore unreadable to both sibling bindings and the unprivileged engine. The owning Tool must
explicitly chmod it or copy it into a group-readable workspace path before an engine file/storage
export. Exact binding-authorized private-file export is post-MVP; the supervisor does not retain a
DAC-bypass capability or privileged helper for it.

Managed Tool input is written through the child's bounded stdin, and its result is one
length-prefixed frame on a fresh anonymous descriptor 3. There is no Tool-visible request/result
directory or predictable IPC pathname. The supervisor reads the declared frame length rather than
waiting for EOF, and gives post-child pipes a bounded settlement deadline. Concurrent bindings
therefore cannot read, precreate, or replace one another's invocation transport even though they
share the workspace group.

Verified bundle modules are installed supervisor-owned and mode `0640`: Tool identities may read
the shared immutable module, but cannot rewrite it after descriptor/digest verification. The Tool
directory itself is not group-writable. This is intentionally separate from the collaborative
workspace umask.

A root-owned preload constructor reapplies `PR_SET_DUMPABLE=0` inside dynamic Node/bash processes
after `exec` resets it; the supervisor assigns this environment last so a customer secret cannot
shadow it. That is defense in depth, not the cross-binding boundary: CI also execs a statically
linked secret-bearing helper (which cannot load the constructor) and proves different managed and
additional-sandbox UIDs cannot read its `/proc/{environ,fd,mem}` surfaces. An inherited
seccomp fence denies `setsid` and `setpgid`, so a forked Tool cannot leave the supervisor-owned
process group. The supervisor kills and reaps that group after normal leader exit as well as on a
deadline or cancellation, so a successful Tool cannot leave a generation-lifetime background
daemon. Both the image build and CI scan the completed filesystem, and CI proves the secret
boundary and process-group fence, so a base-package update cannot silently add a second privilege
transition.

Engine file writes and storage imports never stage a predictable file in the group-writable
workspace. They write and sync under supervisor-only `/var/environment/file-staging`, whose setgid bit
assigns the shared workspace GID while leaving the directory itself inaccessible to Tools, then
atomically link or rename mode-`0660` files into the live workspace. The image and runtime assert
that staging and `/workspace` share a device and GID; an unexpected mount layout fails the
operation rather than falling back to a raceable copy path.

This is the deliberately reduced MVP guest boundary, not a claim that uid separation defeats a
guest-kernel or guest-root exploit. Such a compromise can violate confidentiality and availability
for processes and workspace data in that same root tree. PID/mount namespaces and delegated
cgroup-v2 per operation are post-MVP hardening; adding a persistent root/CAP_SYS_ADMIN protocol
parser would make the current boundary worse. The externally enforced MicroVM connector, VPC and
allowlist gateway remain authoritative after guest-root compromise, so guest code still cannot
widen the sealed egress class or destination grant.

## Live no-respawn release gate

The image workflow accepts a full lowercase source SHA only while running the exact
`release/sha-<sha>` tag and rejects any checkout mismatch before building. This prevents queued
dev and production approvals from silently selecting different moving `main` revisions. It runs
`environment-lambda image canary` against the exact immutable image version it just published. This
opt-in operator path is not part of Brain's protocol or any Tool input;
normal production target bootstrap always omits it. The canary writes and syncs one marker, waits
for the exact terminal receipt, fsyncs that receipt in supervisor-private state, writes it to the
provider WebSocket, then deliberately aborts `environment-guest`. Promotion fails unless the same target
repeatedly returns HTTP 502 while `GetMicrovm` is observed, continues returning 502 after an
explicit suspend/resume, and refuses the exact operation replay. The target is terminated in the
cleanup path. This tests the provider assumption behind the memory-only guest operation registry:
an application crash is physical-generation loss, never an invitation to run the same effect in a
replacement process under the old target identity.

The same dev workflow then launches and terminates the exact image sequentially through every
connector class. The `none` target must fail DNS resolution and direct TCP to every
connector-routed canonical IPv4 special-use/public fixture as well as the known-live private
gateway listener. Provider-local loopback and IMDS sockets do not traverse a VPC connector and are
not connector-policy evidence: separate image gates require the loopback Environment endpoint's
generation bearer, and production `RunMicrovm` requests attach no execution role from which IMDS
could return AWS credentials. The `allowlist` target must fail the same direct DNS/TCP paths, reach
only that fixed listener, and receive exact `407` and `403` responses for missing and invalid
CONNECT capabilities. These negative canaries do not receive the KMS signing permission used by
the production Environment. The `public` target attempts TCP connections to every connector-routed
canonical IPv4 special-use fixture and fails if any succeeds, then requires canonical public IPv4
controls to remain reachable so a broken connector cannot produce a pass. It also requires source
denial from the website, both public API planes, and both customer-Environment API Gateway hosts. For each
customer-Environment host it exercises a WSS `$connect` handshake and an unauthenticated Management API
request; it does not claim that the public API Gateway TCP endpoint is unreachable.

The workflow reads host-only `AEX_CUSTOMER_ENVIRONMENT_DEV_HOST` and
`AEX_CUSTOMER_ENVIRONMENT_PRD_HOST`, the three connector identities, and the bare fixed-private
`ENVIRONMENT_EGRESS_GATEWAY_AUTHORITY` from the protected `dev` GitHub environment. Operators install the
host values during dormant bootstrap by extracting the hostname from each Platform
`customer_environment_websocket_url` Terraform output; the gateway authority and connector identities
come from the corresponding dev plane outputs. They are environment configuration, not workflow
dispatch inputs, and full callback URLs or grants are never passed to or logged by the canary.
This is behavioral coverage only: an unreachable destination does not reveal which layer rejected
it. The plane's exact Terraform/NACL plan remains the authoritative rule proof. Every canary always
terminates its known target.
