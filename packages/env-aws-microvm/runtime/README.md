<h1 align="center">Environments</h1>

<p align="center"><strong>The default runtime for Brain tools.</strong></p>
<p align="center">
  A Linux guest, curated tool image, and AWS Lambda MicroVM adapter for Brain's public Environment ports.
</p>
<p align="center">
  <a href="https://aex.dev">Aex</a> ·
  <a href="https://github.com/aexhq/brain">Brain</a> ·
  <a href="image/README.md">Image</a> ·
  <a href="gateway/README.md">Gateway</a> ·
  <a href="https://discord.gg/Qk2YnHMHVb">Discord</a>
</p>

Environments implements the Environment ports owned by Brain. It consumes one immutable Brain revision, so wire
contract changes start in [`aexhq/brain`](https://github.com/aexhq/brain) before the pin changes
here.

## Components

| Component | Purpose |
| --- | --- |
| [`environment-core`](crates/environment-core) | Contract-neutral operation, target, generation, connector, and cleanup state machines |
| [`environment-wire`](crates/environment-wire) | Private transport framing for the production Environment |
| [`environment-guest`](crates/environment-guest) | WebSocket guest, tool runner, bounded output, jobs, and live file access |
| [`environment-brain-aws`](crates/environment-brain-aws) | Lambda MicroVM implementation of Brain's receipt and capability ports |
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
[AWS adapter contract](crates/environment-brain-aws/README.md) for runtime details. Hosted Brain
composition belongs to Aex; this repository has no standalone or hosted Brain image.

Licensed under [MIT](LICENSE).
