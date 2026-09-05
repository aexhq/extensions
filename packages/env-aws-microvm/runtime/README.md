<h1 align="center">Environments</h1>

<p align="center"><strong>The default runtime for Brain tools.</strong></p>
<p align="center">
  A Linux guest, curated tool image, and AWS Lambda MicroVM adapter for the public Environment v1 contract.
</p>
<p align="center">
  <a href="https://aex.dev">Aex</a> ·
  <a href="https://github.com/aexhq/brain">Brain</a> ·
  <a href="image/README.md">Image</a> ·
  <a href="gateway/README.md">Gateway</a> ·
  <a href="https://discord.gg/Qk2YnHMHVb">Discord</a>
</p>

Environments implements Brain's public `environment/v1` JSON contract. The loopback driver keeps
Environment configuration from `setup`, consumes complete Tool manifests at `attach`, and accepts
only the versioned `aex_official_tool` implementation descriptors it knows how to resolve. It
verifies each manifest against the publisher-built runtime registry before any bundle reaches a
guest.

The MVP placement and attachment directory is task-local. If the Brain task and its essential
driver sidecar restart, existing external Environment bindings become unreachable; Brain does not
replay `setup` or `attach`, and callers must create a new Environment/session. Provider hard
lifetimes still bound abandoned MicroVMs. This matches the MVP's explicit exclusion of distributed
command delivery and recovery; durable cross-task Environment recovery requires a shared directory
in a later contract.

## Components

| Component | Purpose |
| --- | --- |
| [`environment-core`](crates/environment-core) | Contract-neutral operation, target, generation, connector, and cleanup state machines |
| [`environment-wire`](crates/environment-wire) | Private transport framing for the production Environment |
| [`environment-guest`](crates/environment-guest) | WebSocket guest, tool runner, bounded output, jobs, and live file access |
| [`aws-microvm-controller`](crates/aws-microvm-controller) | Lambda MicroVM implementation of the provider lifecycle and capability ports |
| [`environment-lambda`](crates/environment-lambda) | Image publication, lifecycle controls, and hosted runtime checks |
| [`environment-egress-gateway`](crates/environment-egress-gateway) | Signed-capability HTTP CONNECT allowlist gateway |
| [`environment-driver`](crates/environment-driver) | Authenticated generic dispatch and AWS provider adapter |
| [`gateway`](gateway) | Low-privilege egress gateway image and deployment contract |
| [`driver`](driver) | Hosted Environment driver image |
| [`image`](image) | Curated Linux tool image |
| [`scripts`](scripts) | Node bundle and guest security conformance fixtures |

## Development

The guest is Linux-only and uses process groups, signals, and `/proc`.

```sh
npm run build --workspace @aexhq/tools
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
node scripts/test-tool-runner.mjs
```

CI also builds the Linux image and proves that neither Tool identity class can reach the supervisor
control listener. Production publishes only the immutable egress-gateway image and plane-local
Lambda MicroVM images.

The hosted provider bridge is built independently from this runtime root:

```sh
docker build -f driver/Dockerfile -t environment-driver:dev .
```

Read the [tool image guide](image/README.md), [egress gateway contract](gateway/README.md), or
[AWS adapter contract](crates/aws-microvm-controller/README.md) for runtime details. Hosted Brain
composition belongs to Aex; this repository has no standalone or hosted Brain image.

Licensed under [MIT](LICENSE).
