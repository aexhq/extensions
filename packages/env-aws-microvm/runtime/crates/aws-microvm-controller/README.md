# AWS Environment adapter

`environment-brain-aws` is the production, in-process implementation of Brain's canonical Environment ports.
Aex owns the hosted service composition; this crate does not publish or start a second Brain
server. Construct it with `AwsMicrovmEnvironment::from_env()`, attach the composition-owned
`SecretDeliveryPort` once with `attach_secret_delivery`, and register the same `Arc<AwsMicrovmEnvironment>` as
`EnvironmentPort`, `SessionPreparationPort`, `SandboxFilesPort`, and `SandboxControlPort`.

Hosted MVP targets have one physical resource class: 0.5 baseline vCPU and exactly 1,024 MiB.
The adapter's capacity charge and the image publisher use the same compile-time constant; there
is no production setting or image-publisher flag that can select a different shape.

## Runtime environment

Startup is fail-closed for the production identity and region inputs below.

| Environment | Contract |
| --- | --- |
| `AWS_REGION` | Optional only because the AWS SDK defines it; omission selects `us-east-1`, and every other region is rejected for MVP |
| `ENVIRONMENT_IMAGE` | Required Lambda MicroVM image name/ARN identity |
| `ENVIRONMENT_IMAGE_VERSION` | Required immutable plane-local image version |
| `ENVIRONMENT_REGISTRY_TABLE` | Required plane-local DynamoDB target/definition registry |
| `ENVIRONMENT_MAX_MATERIALIZED_MIB` | Required positive multiple of 1,024; plane allocations must be coordinated so their account-region sum stays within the provider memory quota |
| `ENVIRONMENT_NETWORK_CONNECTOR_NONE` | Required restricted connector ARN |
| `ENVIRONMENT_NETWORK_CONNECTOR_ALLOWLIST` | Required allowlist-gateway connector ARN |
| `ENVIRONMENT_NETWORK_CONNECTOR_PUBLIC` | Required direct-public connector ARN |
| `ENVIRONMENT_CAPABILITY_SIGNING_KEY_ID` | Required KMS P-256 signing-key ARN; the task needs Sign only |
| `ENVIRONMENT_EGRESS_GATEWAY_AUTHORITY` | Required bare `host:port` (production uses the fixed private NLB IPv4 and port 8443); schemes, paths, and credentials are rejected |
| `ENVIRONMENT_BUNDLE_CACHE_MAX_MIB` | Optional verified-bundle memory ceiling, default 128, valid 16–512; hosted planes pin 128 |
| `ENVIRONMENT_BUNDLE_FETCH_MAX_MIB` | Optional subset of that ceiling available to concurrent cold fetches, default 32, valid 16–cache ceiling; hosted planes pin 32 |

Lifecycle admission is controlled by eight validated optional variables. Defaults match the
observed reduced `us-east-1` account; maximums match the public operation ceilings.

| Operation | Rate environment (default / max) | Burst environment (default / max) |
| --- | --- | --- |
| Run | `ENVIRONMENT_PROVIDER_RUN_RATE_PER_SECOND` (1 / 5) | `ENVIRONMENT_PROVIDER_RUN_BURST` (1 / 5) |
| Resume | `ENVIRONMENT_PROVIDER_RESUME_RATE_PER_SECOND` (5 / 5) | `ENVIRONMENT_PROVIDER_RESUME_BURST` (5 / 5) |
| Suspend | `ENVIRONMENT_PROVIDER_SUSPEND_RATE_PER_SECOND` (2 / 2) | `ENVIRONMENT_PROVIDER_SUSPEND_BURST` (2 / 2) |
| Terminate | `ENVIRONMENT_PROVIDER_TERMINATE_RATE_PER_SECOND` (10 / 10) | `ENVIRONMENT_PROVIDER_TERMINATE_BURST` (10 / 10) |

There is deliberately no `ENVIRONMENT_STORAGE_BUCKET` or external executor token. Bundles and objects are
fetched or uploaded only through Brain-issued, short-lived one-purpose HTTPS authorities. Secret
values use the attached one-redemption callback and never enter environment configuration, the
target registry, request logging, or process arguments.

Object authorities terminate in this trusted adapter. After streaming and verifying a download,
Environments sends the guest only the immutable object identity and staged bytes; presigned URLs and
headers never enter the MicroVM. Exports likewise stream directly from authenticated guest ingress
to the one-purpose PUT authority.

Redeemed secret material repeats Brain's canonical 128-name, 8-KiB/value and 4-KiB whole JCS
document bounds, and at most four redemptions/installations are resident at once. Environments removes the
bearer before invoking the asynchronous redemption callback, posts the
material with a bounded exact retry, and then zeroizes its local values. It never retains secret
values for a dormant session. The guest supervisor owns a confirmed immutable session union for
the physical generation and injects only each binding's declared subset. A lost or uncertain
installation response therefore requires Brain to issue a fresh single-use capability; the guest
replays an already-installed exact union idempotently before any Tool effect.

Verified bundle bytes are held only in a bounded LRU process cache. Immutable preparation metadata
records descriptors and digests but does not pin resident bytes; only an active fetch or guest
installation holds a byte reference. Idle entries may therefore be evicted under pressure, and a
later miss returns `capability_unavailable` before dispatch so Brain can re-prepare with a fresh
short-lived fetch. The default 128 MiB ceiling includes both resident bytes and every cold fetch's
declared maximum; concurrent cold fetches also share the narrower 32 MiB admission budget and the
bundle cache has a 4,096-entry bound. The separate immutable preparation-metadata LRU is fixed at
64 MiB and 16,384 sessions; an evicted session likewise requires a fresh preparation before any
materialization or effect. Environments consumes Brain's canonical 4 MiB per-Tool bundle
limit; the private guest transport keeps 16 MiB of payload headroom, matching Brain's aggregate
session-bundle ceiling so it cannot become a narrower limit. Cold guest installation is limited to
four concurrent buffered requests, while a replay whose bytes remain resident is network-free.
The guest's exact bundle/binding installation remains idempotent. Authorities and bundle bytes
never enter the DynamoDB registry.

The registry table uses `root_id` (string) as partition key and `target_key` (string) as sort key.
It is pay-per-request. Target creation and the plane capacity counter share one DynamoDB
transaction; established managed submit calls perform no registry read or write. Target tombstones
do not set a TTL: they remain until the owning root's explicit, retryable purge so a terminated
additional sandbox ID can never rematerialize after an arbitrary delay. Every installed target
returns the conservative physical hard deadline derived before provider dispatch. Brain journals
that deadline and schedules an exact inspect/terminate even without customer traffic; the
confirmed terminal transition and capacity refund are one transaction.

More precisely, established **submit** calls use Brain's projected physical target and stay off
DynamoDB. Observe, cancel, and terminal acknowledgement carry the exact rooted `SandboxTarget`,
generation, and physical target reference from the journaled receipt. Those control calls perform
one exact registry lookup—not a scan or reverse index—so a persistent guest 502 can terminate the
known MicroVM, transition the rooted row to `gone`, and refund capacity only after provider-confirmed
absence.

The first submit has a stricter lost-receipt rule. Once the guest Submit RPC is attempted, persistent
supervisor/endpoint loss returns non-retryable `operation_unknown` while leaving that physical
generation installed and capacity-fenced. A retry therefore reaches the same generation and can
never execute the intent in a replacement VM. Brain durably revokes submit replay, observes the
exact rooted target and retries default-target dematerialization until termination/refund is
confirmed; only a later explicit generation intent may replace it. Failures before Submit dispatch
may still retire and safely replace a target because no Tool effect could have started.

Before `RunMicrovm`, the materializing target row also stores a stable provider `clientToken` and
the byte-exact closed Run request (resolved image/version, connector and run-hook payload). A short
attempt-owner lease may move between Environment processes, but recovery always replays those same bytes;
AWS's Run idempotency contract therefore returns the same MicroVM instead of allocating a second
one. A first dispatch or exact recovery replay is admitted only during the four-minute window
sealed into that row. After the window, a row known never to have reached the provider is refunded;
a row whose dispatch may have happened keeps its capacity fence until the full eight-hour provider
lifetime plus skew has elapsed. This prevents a delayed recovery launch from outliving and
understating the plane's charged memory. Sensitive allowlist capability bytes are binary,
Debug-redacted, never logged, and removed in the same CAS that installs the target. If the exact
request is corrupt or conflicts with its immutable target seal, Environments fails closed and retains the
conservative uncertainty fence rather than refunding capacity unsafely.

The environment extension owns additional-sandbox inventory and capacity limits. It never offers a
broad unauthenticated list operation: create, inspect, execute, file access, and terminate require
the exact rooted target and generation fence.

A logical environment's connector and physical network ceiling are sealed from its preparation,
never from the first narrower Tool call. The extension rejects an operation that widens that seal
using Brain's canonical subset rules. MVP does not claim per-process enforcement of a Tool's
narrower network declaration inside a shared logical environment: bindings share its connector.
Code that requires a physically narrower connector must use a separately declared environment or
an explicitly created additional sandbox. There is no connector fallback in either path.

Sandbox status is an on-demand provider observation, not a metering stream. A returned
`suspended` state is authoritative only at the instant of the `GetMicrovm` response. The provider
does not expose `suspended_at` or retained workspace bytes, and `changed_at_ms` records the latest
durable target status/install observation; it must not be interpreted as the time auto-suspend
occurred. Environments does not predict the provider's asynchronous idle transition or run a polling
service.
