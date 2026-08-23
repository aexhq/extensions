use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::capability::{MAX_ENCODED_TOKEN_BYTES, VerifiedCapability, now_ms, verify_token};
use crate::config::Config;
use crate::policy::{AuthorizedTarget, authorize};

const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;
const CONNECT_HEADER_HEADROOM_BYTES: usize = 2 * 1024;
pub async fn serve(config: Config) -> io::Result<()> {
    let listener = TcpListener::bind(config.listen).await?;
    let health_listener = TcpListener::bind(config.health_listen).await?;
    let config = Arc::new(config);
    let tunnels = Arc::new(Semaphore::new(config.max_connections));
    let roots = Arc::new(RootQuotas::new(config.max_connections_per_root));
    // Active tunnels do not consume setup slots. The independent health listener remains
    // responsive even when every slow/incomplete setup slot is occupied.
    let setups = Arc::new(Semaphore::new(config.max_pending_setups));
    tracing::info!(listen = %listener.local_addr()?, "egress gateway listening");
    tracing::info!(listen = %health_listener.local_addr()?, "egress gateway health listening");
    tokio::try_join!(
        serve_proxy(listener, config.clone(), tunnels, setups, roots),
        serve_health(health_listener, config.setup_timeout),
    )?;
    Ok(())
}

async fn serve_proxy(
    listener: TcpListener,
    config: Arc<Config>,
    tunnels: Arc<Semaphore>,
    setups: Arc<Semaphore>,
    roots: Arc<RootQuotas>,
) -> io::Result<()> {
    loop {
        let permit = setups
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore remains open");
        let (stream, peer) = listener.accept().await?;
        let config = config.clone();
        let tunnels = tunnels.clone();
        let roots = roots.clone();
        tokio::spawn(async move {
            if let Err(error) = handle(stream, config, tunnels, roots, permit).await {
                tracing::debug!(%peer, error = %error, "gateway connection closed");
            }
        });
    }
}

async fn serve_health(listener: TcpListener, timeout: Duration) -> io::Result<()> {
    loop {
        let (stream, peer) = listener.accept().await?;
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + timeout;
            if let Err(error) = handle_health_connection(stream, deadline).await {
                tracing::debug!(%peer, error = %error, "gateway health connection closed");
            }
        });
    }
}

async fn handle(
    mut client: TcpStream,
    config: Arc<Config>,
    tunnels: Arc<Semaphore>,
    roots: Arc<RootQuotas>,
    setup_permit: OwnedSemaphorePermit,
) -> Result<(), ProxyError> {
    client.set_nodelay(true)?;
    let deadline = tokio::time::Instant::now() + config.setup_timeout;
    let first = read_u8(&mut client, deadline).await?;
    match first {
        b'C' => {
            handle_http(
                client,
                b'C',
                &config,
                tunnels,
                roots,
                setup_permit,
                deadline,
            )
            .await
        }
        b'G' => handle_health(client, b'G', deadline).await,
        // RFC 1929 can carry at most 510 authentication bytes, less than a normal signed Aex
        // capability even for one destination. Advertising that path would be false support.
        0x05 => Err(ProxyError::UnsupportedProtocol),
        _ => Err(ProxyError::MalformedRequest),
    }
}

async fn handle_health_connection(
    mut client: TcpStream,
    deadline: tokio::time::Instant,
) -> Result<(), ProxyError> {
    client.set_nodelay(true)?;
    let first = read_u8(&mut client, deadline).await?;
    if first != b'G' {
        return Err(ProxyError::MalformedRequest);
    }
    handle_health(client, first, deadline).await
}

async fn handle_health(
    mut client: TcpStream,
    first: u8,
    deadline: tokio::time::Instant,
) -> Result<(), ProxyError> {
    let request = read_http_header(&mut client, first, deadline).await?;
    let line = request.lines().next().unwrap_or_default();
    if line != "GET /healthz HTTP/1.1" && line != "GET /healthz HTTP/1.0" {
        write_http_error(&mut client, 404, "Not Found").await?;
        return Ok(());
    }
    client
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
        .await?;
    Ok(())
}

async fn handle_http(
    mut client: TcpStream,
    first: u8,
    config: &Config,
    tunnels: Arc<Semaphore>,
    roots: Arc<RootQuotas>,
    setup_permit: OwnedSemaphorePermit,
    deadline: tokio::time::Instant,
) -> Result<(), ProxyError> {
    // Every rejection funnels through one place, so a new check cannot forget its HTTP answer.
    let established = match connect_setup(
        &mut client,
        first,
        config,
        tunnels,
        roots,
        setup_permit,
        deadline,
    )
    .await
    {
        Ok(established) => established,
        Err(error) => {
            match error.http_response() {
                Some((407, _)) => {
                    client
                            .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\nConnection: close\r\nProxy-Authenticate: Bearer\r\n\r\n")
                            .await?;
                }
                Some((status, reason)) => {
                    write_http_error(&mut client, status, reason).await?;
                }
                None => {}
            }
            return Err(error);
        }
    };
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    tunnel(
        client,
        established.upstream,
        established.target,
        config,
        deadline,
        established.capability_expires_at_ms,
        established.guards,
    )
    .await
}

/// Everything a CONNECT needs before the 200 is written: the connected upstream, the authorized
/// target, and the capacity guards the tunnel must hold.
struct EstablishedConnect {
    upstream: TcpStream,
    target: AuthorizedTarget,
    capability_expires_at_ms: u64,
    guards: TunnelGuards,
}

async fn connect_setup(
    client: &mut TcpStream,
    first: u8,
    config: &Config,
    tunnels: Arc<Semaphore>,
    roots: Arc<RootQuotas>,
    setup_permit: OwnedSemaphorePermit,
    deadline: tokio::time::Instant,
) -> Result<EstablishedConnect, ProxyError> {
    let request = read_http_header(client, first, deadline).await?;
    let capability = authenticate(&request, config)?;
    let parsed = parse_connect(&request)?;
    let capability_expires_at_ms = capability.capability.expires_at_ms;
    let root_permit = roots
        .acquire(&capability.capability.root_id)
        .ok_or(ProxyError::RootCapacity)?;
    let target = tokio::time::timeout_at(
        deadline,
        authorize(
            &capability,
            &config.deny,
            &config.resolver,
            parsed.authority,
            parsed.port,
        ),
    )
    .await
    .map_err(|_| ProxyError::SetupTimeout)??;
    let tunnel_permit = tokio::time::timeout_at(deadline, tunnels.acquire_owned())
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or(ProxyError::Capacity)?;
    // The setup slot converts into a tunnel slot here, before the upstream dial: a slow dial
    // must consume relay capacity, not the bounded handshake budget.
    drop(setup_permit);
    let upstream = tokio::time::timeout_at(deadline, TcpStream::connect(target.address))
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or(ProxyError::Upstream)?;
    Ok(EstablishedConnect {
        upstream,
        target,
        capability_expires_at_ms,
        guards: TunnelGuards {
            _global: tunnel_permit,
            _root: root_permit,
        },
    })
}

/// Verifies the bearer capability on the CONNECT request.
fn authenticate(request: &str, config: &Config) -> Result<VerifiedCapability, ProxyError> {
    let parsed = parse_connect(request)?;
    let token = parsed.token.ok_or(ProxyError::Authentication)?;
    Ok(verify_token(&token, &config.public_key, now_ms())?)
}

struct ConnectRequest<'a> {
    authority: &'a str,
    port: u16,
    token: Option<String>,
}

fn parse_connect(request: &str) -> Result<ConnectRequest<'_>, ProxyError> {
    let mut lines = request.split("\r\n");
    let line = lines.next().ok_or(ProxyError::MalformedRequest)?;
    let mut parts = line.split(' ');
    if parts.next() != Some("CONNECT") {
        return Err(ProxyError::MalformedRequest);
    }
    let authority = parts.next().ok_or(ProxyError::MalformedRequest)?;
    if parts.next() != Some("HTTP/1.1") || parts.next().is_some() {
        return Err(ProxyError::MalformedRequest);
    }
    let (host, port) = parse_authority(authority)?;
    let mut token = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            return Err(ProxyError::MalformedRequest);
        }
        let (name, value) = line.split_once(':').ok_or(ProxyError::MalformedRequest)?;
        if name.eq_ignore_ascii_case("proxy-authorization") {
            if token.is_some() {
                return Err(ProxyError::MalformedRequest);
            }
            token = Some(parse_proxy_authorization(value.trim())?);
        }
    }
    Ok(ConnectRequest {
        authority: host,
        port,
        token,
    })
}

fn parse_proxy_authorization(value: &str) -> Result<String, ProxyError> {
    if value.len() > MAX_HTTP_HEADER_BYTES - CONNECT_HEADER_HEADROOM_BYTES {
        return Err(ProxyError::Authentication);
    }
    if let Some(token) = value.strip_prefix("Bearer ") {
        if token.is_empty() || token.len() > MAX_ENCODED_TOKEN_BYTES {
            return Err(ProxyError::Authentication);
        }
        return Ok(token.to_owned());
    }
    let encoded = value
        .strip_prefix("Basic ")
        .ok_or(ProxyError::Authentication)?;
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|_| ProxyError::Authentication)?;
    let decoded = std::str::from_utf8(&decoded).map_err(|_| ProxyError::Authentication)?;
    let token = decoded
        .strip_prefix("aex:")
        .ok_or(ProxyError::Authentication)?;
    if token.is_empty() || token.len() > MAX_ENCODED_TOKEN_BYTES {
        return Err(ProxyError::Authentication);
    }
    Ok(token.to_owned())
}

fn parse_authority(authority: &str) -> Result<(&str, u16), ProxyError> {
    if authority.starts_with('[') {
        return Err(ProxyError::Policy(
            crate::policy::PolicyError::Ipv6Unsupported,
        ));
    }
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or(ProxyError::MalformedRequest)?;
    if host.is_empty() || host.contains(':') {
        return Err(ProxyError::MalformedRequest);
    }
    let port = port.parse().map_err(|_| ProxyError::MalformedRequest)?;
    Ok((host, port))
}

async fn tunnel(
    mut client: TcpStream,
    mut upstream: TcpStream,
    target: AuthorizedTarget,
    config: &Config,
    setup_deadline: tokio::time::Instant,
    capability_expires_at_ms: u64,
    _guards: TunnelGuards,
) -> Result<(), ProxyError> {
    client.set_nodelay(true)?;
    upstream.set_nodelay(true)?;
    let budget = Arc::new(ByteBudget::new(config.max_relay_bytes));
    if let Some(expected_sni) = target.expected_sni {
        let (hello, actual_sni) =
            crate::tls::read_client_hello(&mut client, setup_deadline).await?;
        if actual_sni != expected_sni {
            return Err(ProxyError::SniMismatch);
        }
        if !budget.consume(hello.len() as u64) {
            return Err(ProxyError::RelayLimit);
        }
        tokio::time::timeout_at(setup_deadline, upstream.write_all(&hello))
            .await
            .map_err(|_| ProxyError::SetupTimeout)??;
    }
    let remaining = Duration::from_millis(capability_expires_at_ms.saturating_sub(now_ms()));
    if remaining.is_zero() {
        return Err(ProxyError::CapabilityExpired);
    }
    tokio::time::timeout(
        remaining,
        relay(client, upstream, config.idle_timeout, budget),
    )
    .await
    .map_err(|_| ProxyError::CapabilityExpired)??;
    Ok(())
}

async fn relay(
    client: TcpStream,
    upstream: TcpStream,
    idle: Duration,
    budget: Arc<ByteBudget>,
) -> io::Result<()> {
    let (client_read, client_write) = client.into_split();
    let (upstream_read, upstream_write) = upstream.into_split();
    let toward_upstream = copy_with_idle(client_read, upstream_write, idle, budget.clone());
    let toward_client = copy_with_idle(upstream_read, client_write, idle, budget);
    tokio::try_join!(toward_upstream, toward_client)?;
    Ok(())
}

async fn copy_with_idle<R, W>(
    mut reader: R,
    mut writer: W,
    idle: Duration,
    budget: Arc<ByteBudget>,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = tokio::time::timeout(idle, reader.read(&mut buffer))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "idle timeout"))??;
        if read == 0 {
            writer.shutdown().await?;
            return Ok(());
        }
        if !budget.consume(read as u64) {
            return Err(io::Error::other("relay byte limit reached"));
        }
        tokio::time::timeout(idle, writer.write_all(&buffer[..read]))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "idle timeout"))??;
    }
}

struct ByteBudget {
    remaining: AtomicU64,
}

impl ByteBudget {
    fn new(bytes: u64) -> Self {
        Self {
            remaining: AtomicU64::new(bytes),
        }
    }

    fn consume(&self, bytes: u64) -> bool {
        self.remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(bytes)
            })
            .is_ok()
    }
}

struct RootQuotas {
    max: usize,
    active: Mutex<HashMap<String, usize>>,
}

impl RootQuotas {
    fn new(max: usize) -> Self {
        Self {
            max,
            active: Mutex::new(HashMap::new()),
        }
    }

    fn acquire(self: &Arc<Self>, root_id: &str) -> Option<RootPermit> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let count = active.entry(root_id.to_owned()).or_default();
        if *count >= self.max {
            return None;
        }
        *count += 1;
        Some(RootPermit {
            quotas: self.clone(),
            root_id: root_id.to_owned(),
        })
    }
}

struct RootPermit {
    quotas: Arc<RootQuotas>,
    root_id: String,
}

struct TunnelGuards {
    _global: OwnedSemaphorePermit,
    _root: RootPermit,
}

impl Drop for RootPermit {
    fn drop(&mut self) {
        let mut active = self
            .quotas
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(count) = active.get_mut(&self.root_id) else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            active.remove(&self.root_id);
        }
    }
}

async fn read_http_header(
    stream: &mut TcpStream,
    first: u8,
    deadline: tokio::time::Instant,
) -> Result<String, ProxyError> {
    let mut bytes = vec![first];
    while !bytes.ends_with(b"\r\n\r\n") {
        if bytes.len() >= MAX_HTTP_HEADER_BYTES {
            return Err(ProxyError::RequestTooLarge);
        }
        bytes.push(read_u8(stream, deadline).await?);
    }
    String::from_utf8(bytes).map_err(|_| ProxyError::MalformedRequest)
}

async fn read_u8(stream: &mut TcpStream, deadline: tokio::time::Instant) -> Result<u8, ProxyError> {
    let mut byte = [0_u8; 1];
    tokio::time::timeout_at(deadline, stream.read_exact(&mut byte))
        .await
        .map_err(|_| ProxyError::SetupTimeout)??;
    Ok(byte[0])
}

async fn write_http_error(stream: &mut TcpStream, status: u16, reason: &str) -> io::Result<()> {
    stream
        .write_all(
            format!("HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
}

#[derive(Debug, thiserror::Error)]
enum ProxyError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("setup timed out")]
    SetupTimeout,
    #[error("request is malformed")]
    MalformedRequest,
    #[error("proxy protocol is unsupported")]
    UnsupportedProtocol,
    #[error("request is too large")]
    RequestTooLarge,
    #[error("authentication failed")]
    Authentication,
    #[error("relay connection capacity is exhausted")]
    Capacity,
    #[error("root connection quota is exhausted")]
    RootCapacity,
    #[error("relay byte limit reached")]
    RelayLimit,
    #[error("capability expired while the tunnel was open")]
    CapabilityExpired,
    #[error("upstream connection failed")]
    Upstream,
    #[error("TLS SNI does not match requested authority")]
    SniMismatch,
    #[error(transparent)]
    Capability(#[from] crate::capability::CapabilityError),
    #[error(transparent)]
    Policy(#[from] crate::policy::PolicyError),
    #[error(transparent)]
    Tls(#[from] crate::tls::TlsError),
}

impl ProxyError {
    /// The HTTP answer a pre-tunnel rejection owes the client. `None` means the failure has no
    /// HTTP context (broken socket) or happened after the 200 was written (tunnel-phase errors).
    fn http_response(&self) -> Option<(u16, &'static str)> {
        match self {
            ProxyError::MalformedRequest => Some((400, "Bad Request")),
            ProxyError::Authentication => Some((407, "Proxy Authentication Required")),
            ProxyError::Capability(_) | ProxyError::Policy(_) => Some((403, "Forbidden")),
            ProxyError::RootCapacity => Some((429, "Too Many Requests")),
            ProxyError::RequestTooLarge => Some((431, "Request Header Fields Too Large")),
            ProxyError::Upstream => Some((502, "Bad Gateway")),
            ProxyError::Capacity => Some((503, "Service Unavailable")),
            ProxyError::SetupTimeout => Some((504, "Gateway Timeout")),
            ProxyError::Io(_)
            | ProxyError::UnsupportedProtocol
            | ProxyError::RelayLimit
            | ProxyError::CapabilityExpired
            | ProxyError::SniMismatch
            | ProxyError::Tls(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use p256::ecdsa::SigningKey;

    use super::*;

    #[test]
    fn connect_parser_requires_exact_method_authority_and_single_bearer() {
        let parsed = parse_connect(
            "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Bearer abc.def\r\n\r\n",
        )
        .unwrap();
        assert_eq!(parsed.authority, "example.com");
        assert_eq!(parsed.port, 443);
        assert_eq!(parsed.token.as_deref(), Some("abc.def"));
        let basic = STANDARD.encode("aex:abc.def");
        let request = format!(
            "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic {basic}\r\n\r\n"
        );
        let parsed = parse_connect(&request).unwrap();
        assert_eq!(parsed.token.as_deref(), Some("abc.def"));
        for request in [
            "GET example.com:443 HTTP/1.1\r\n\r\n",
            "CONNECT example.com HTTP/1.1\r\n\r\n",
            "CONNECT [::1]:443 HTTP/1.1\r\n\r\n",
            "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic x\r\n\r\n",
            "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Bearer a\r\nProxy-Authorization: Bearer b\r\n\r\n",
        ] {
            assert!(parse_connect(request).is_err(), "{request}");
        }
    }

    #[test]
    fn bearer_and_basic_share_one_usable_encoded_token_bound() {
        let largest = "a".repeat(MAX_ENCODED_TOKEN_BYTES);
        assert_eq!(
            parse_proxy_authorization(&format!("Bearer {largest}")).unwrap(),
            largest
        );
        let basic = STANDARD.encode(format!("aex:{largest}"));
        assert_eq!(
            parse_proxy_authorization(&format!("Basic {basic}")).unwrap(),
            largest
        );
        let basic_header_bytes = "Proxy-Authorization: Basic ".len() + basic.len() + 2;
        assert!(
            basic_header_bytes + CONNECT_HEADER_HEADROOM_BYTES <= MAX_HTTP_HEADER_BYTES,
            "largest Basic capability must leave CONNECT request headroom"
        );

        let too_large = "a".repeat(MAX_ENCODED_TOKEN_BYTES + 1);
        assert!(parse_proxy_authorization(&format!("Bearer {too_large}")).is_err());
        let basic = STANDARD.encode(format!("aex:{too_large}"));
        assert!(parse_proxy_authorization(&format!("Basic {basic}")).is_err());
    }

    #[tokio::test]
    async fn relay_has_bounded_idle_and_preserves_bytes() {
        let (mut client, gateway_client) = tokio::io::duplex(1024);
        let (gateway_upstream, mut upstream) = tokio::io::duplex(1024);
        let task = tokio::spawn(async move {
            let toward_upstream = copy_with_idle(
                gateway_client,
                gateway_upstream,
                Duration::from_secs(1),
                Arc::new(ByteBudget::new(5)),
            );
            toward_upstream.await.unwrap();
        });
        client.write_all(b"hello").await.unwrap();
        client.shutdown().await.unwrap();
        let mut bytes = Vec::new();
        upstream.read_to_end(&mut bytes).await.unwrap();
        task.await.unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[tokio::test]
    async fn relay_rejects_one_byte_over_the_total_budget_without_forwarding_that_chunk() {
        let (mut client, gateway_client) = tokio::io::duplex(1024);
        let (gateway_upstream, mut upstream) = tokio::io::duplex(1024);
        let task = tokio::spawn(async move {
            copy_with_idle(
                gateway_client,
                gateway_upstream,
                Duration::from_secs(1),
                Arc::new(ByteBudget::new(4)),
            )
            .await
        });
        client.write_all(b"hello").await.unwrap();
        client.shutdown().await.unwrap();
        assert_eq!(
            task.await.unwrap().unwrap_err().kind(),
            io::ErrorKind::Other
        );
        let mut bytes = Vec::new();
        upstream.read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn root_quota_is_shared_and_released_exactly() {
        let quotas = Arc::new(RootQuotas::new(1));
        let permit = quotas.acquire("root-1").expect("first tunnel");
        assert!(quotas.acquire("root-1").is_none());
        assert!(quotas.acquire("root-2").is_some());
        drop(permit);
        assert!(quotas.acquire("root-1").is_some());
    }

    #[tokio::test]
    async fn independent_health_listener_survives_proxy_setup_saturation() {
        let listen = unused_address();
        let health_listen = unused_address();
        let resolver = hickory_resolver::TokioResolver::builder_tokio()
            .unwrap()
            .build()
            .unwrap();
        let key = SigningKey::from_slice(&[7; 32]).unwrap();
        let config = Config {
            listen,
            health_listen,
            public_key: key.verifying_key().to_owned(),
            resolver,
            deny: crate::policy::DenyPolicy::new(Vec::new(), Vec::new()),
            max_connections: 1,
            max_connections_per_root: 1,
            max_pending_setups: 1,
            max_relay_bytes: 1024 * 1024,
            setup_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(5),
        };
        let server = tokio::spawn(serve(config));
        let mut slow = connect_eventually(listen).await;
        slow.write_all(b"C").await.unwrap();
        // The only proxy setup permit is now blocked waiting for the rest of CONNECT. Health uses
        // a separately accepted socket and must not queue behind it.
        let mut health = connect_eventually(health_listen).await;
        health
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), health.read_to_end(&mut response))
            .await
            .expect("health response was starved")
            .unwrap();
        assert!(
            String::from_utf8(response)
                .unwrap()
                .starts_with("HTTP/1.1 200 OK")
        );
        server.abort();
    }

    fn unused_address() -> std::net::SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    }

    async fn connect_eventually(address: std::net::SocketAddr) -> TcpStream {
        for _ in 0..100 {
            match TcpStream::connect(address).await {
                Ok(stream) => return stream,
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
        panic!("listener {address} did not start")
    }
}
