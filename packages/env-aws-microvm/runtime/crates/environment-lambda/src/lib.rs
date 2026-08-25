//! Host-side mechanics for Environments on **AWS Lambda MicroVMs**.
//!
//! The MicroVM service gives us Firecracker isolation as a product: one VM per session, a
//! Lambda-managed AL2023 base image with our container layer inside it, an authenticated
//! public HTTPS endpoint per VM (JWE in `X-aws-proxy-auth`) and a hard 8-hour retention wall.
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
/// image registration declares and the endpoint auth token is scoped to. `image/supervisor-launcher.c`
/// and `image/environment-boot.sh` carry the same value; the image conformance tests pin them together.
pub const AGENT_PORT: u16 = 8080;

/// The sandbox identity split baked into both images and the C launcher: the trusted Environment
/// supervisor uid/gid and the untrusted Tool uid/gid. The image conformance tests pin the
/// Dockerfiles and `image/supervisor-launcher.c` to these values.
pub const SUPERVISOR_UID: u32 = 1001;
pub const TOOL_UID: u32 = 1000;
pub const TOOL_GID: u32 = 1000;

/// Keep the VM runnable for its complete supported lifetime. Provider auto-suspend can interrupt
/// a live Tool while its caller is waiting on model or child-session work rather than endpoint I/O.
pub const MAX_IDLE_SECONDS: u64 = MAX_DURATION_SECONDS;

/// The provider-hard retention wall: running plus suspended (quota L-B430C318).
pub const MAX_DURATION_SECONDS: u64 = 28_800;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetLifetime {
    pub idle_seconds: u64,
    pub maximum_seconds: u64,
}

impl TargetLifetime {
    pub fn new(idle_seconds: u64, maximum_seconds: u64) -> Result<Self, &'static str> {
        if idle_seconds == 0
            || maximum_seconds == 0
            || idle_seconds > maximum_seconds
            || idle_seconds > MAX_IDLE_SECONDS
            || maximum_seconds > MAX_DURATION_SECONDS
        {
            return Err("AWS target lifetime is outside the provider boundary");
        }
        Ok(Self {
            idle_seconds,
            maximum_seconds,
        })
    }
}

impl Default for TargetLifetime {
    fn default() -> Self {
        Self {
            idle_seconds: MAX_IDLE_SECONDS,
            maximum_seconds: MAX_DURATION_SECONDS,
        }
    }
}

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
