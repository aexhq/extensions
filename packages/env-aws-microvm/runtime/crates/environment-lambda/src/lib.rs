//! Host-side mechanics for Environments on **AWS Lambda MicroVMs**.
//!
//! The MicroVM service gives us Firecracker isolation as a product: one VM per session, a
//! Lambda-managed AL2023 base image with our container layer inside it, an authenticated
//! public HTTPS endpoint per VM (JWE in `X-aws-proxy-auth`), traffic-idle auto-suspend, and a
//! hard 8-hour retention wall (running *plus* suspended).
//!
//! Three parts:
//!
//! - [`image`] — the pipeline: generated Dockerfile + guest binary + boot script packed into a
//!   ZIP, uploaded to S3, registered with `CreateMicrovmImage`/`UpdateMicrovmImage` (AWS runs
//!   the build), and checked against the managed base's deprecation schedule (images expire on
//!   AWS's calendar, not ours).
//! - [`control`] — typed lifecycle calls (`RunMicrovm`/`Get`/`Suspend`/`Resume`/`Terminate`,
//!   `CreateMicrovmAuthToken`) with explicit retryable, throttled, unknown-effect, gone and fatal
//!   classifications. State-changing 5xx/transport failures are never called safe retries.
//! - [`launch`] — exact-replay launch against a durably sealed request, delivery of the
//!   immutable secret-free target seal through the run hook, readiness waiting, and the guest
//!   WebSocket connect. The Environment deliberately has no independent keepalive loop.

pub mod canary;
pub mod control;
pub mod image;
pub mod launch;

/// The default region for the hosted Environment image line.
pub const REGION: &str = "us-east-1";

/// The single guest port: lifecycle hooks, probe, and the ABI WebSocket. Matches the port the
/// image registration declares and the endpoint auth token is scoped to. `image/control-listener.c`
/// and `image/environment-boot.sh` carry the same value; the image conformance tests pin them together.
pub const AGENT_PORT: u16 = 8080;

/// The sandbox identity split baked into both images and the C launcher: the trusted Environment
/// supervisor uid/gid and the untrusted Tool uid/gid. The image conformance tests pin the
/// Dockerfiles and `image/control-listener.c` to these values.
pub const SUPERVISOR_UID: u32 = 1001;
pub const TOOL_UID: u32 = 1000;
pub const TOOL_GID: u32 = 1000;

/// Idle policy: AWS suspends the VM after this much endpoint-traffic silence.
pub const MAX_IDLE_SECONDS: u64 = 180;

/// The provider-hard retention wall: running plus suspended (quota L-B430C318).
pub const MAX_DURATION_SECONDS: u64 = 28_800;

/// Endpoint auth token lifetime. `CreateMicrovmAuthToken` accepts minutes, max 60.
pub const TOKEN_TTL_SECONDS: u64 = 3_600;

/// One `SdkConfig` per process: every AWS client in a command derives from this single load, so
/// region and credential-provider resolution cannot silently diverge between clients.
pub async fn aws_config(region: &str) -> aws_config::SdkConfig {
    aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_owned()))
        .load()
        .await
}

/// Builder for every client that talks to a MicroVM endpoint: no ambient proxy (guest JWE and
/// one-purpose object authorities must never be forwarded through one) and no redirects.
pub fn endpoint_http_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
}
