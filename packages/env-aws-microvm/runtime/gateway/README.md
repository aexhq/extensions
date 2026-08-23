# Environment egress gateway

This private, low-privilege TCP service is the only destination reachable through an `allowlist`
MicroVM connector. It supports HTTP CONNECT, verifies one KMS P-256 capability minted by
the trusted Environment per sandbox generation, resolves the requested host itself, rejects permanent
destination ranges, checks visible TLS SNI for host grants, and relays bounded buffers without
terminating TLS.

It is one control in the production boundary, not a replacement for the restricted VPC, fixed
internal NLB addresses, routes, security groups, network ACLs, fail-closed DNS Firewall, NAT ingress
deny, or normal authentication on public Aex endpoints.

The hosted Environment receives the private listener separately as
`ENVIRONMENT_EGRESS_GATEWAY_AUTHORITY=<fixed-private-NLB-IPv4>:8443`. It is a bare authority, not a URL:
startup rejects schemes, credentials and paths. The signed capability is injected into the tool
environment only, never into process arguments or diagnostic fields.

## Runtime contract

| Environment | Required | Meaning |
| --- | --- | --- |
| `AEX_GATEWAY_LISTEN` | No | Listener, default `0.0.0.0:8080` |
| `AEX_GATEWAY_HEALTH_LISTEN` | No | Independent health listener, default `0.0.0.0:8081`; keep NLB health checks responsive when proxy setup slots are saturated |
| `AEX_GATEWAY_PUBLIC_KEY_PEM` | One key source | Inline KMS P-256 SPKI PEM; appropriate for ECS because it is public data |
| `AEX_GATEWAY_PUBLIC_KEY_DER_BASE64` | One key source | Inline standard-base64 SPKI DER |
| `AEX_GATEWAY_PUBLIC_KEY_FILE` | One key source | PEM or DER file for local/Kubernetes use |
| `AEX_GATEWAY_DENY_HOSTS` | Yes | Comma-separated exact or `*.` extra permanent-deny host patterns; `aex.dev` and `*.aex.dev` are always built in |
| `AEX_GATEWAY_DENY_CIDRS` | No | Extra comma-separated IPv4 CIDRs; the complete built-in IANA special-use table (including private, metadata, transition, documentation, benchmark, multicast and reserved ranges) is always denied |
| `AEX_GATEWAY_MAX_CONNECTIONS` | No | In-flight cap, default 1,024 |
| `AEX_GATEWAY_MAX_CONNECTIONS_PER_ROOT` | No | In-flight root-tree cap across all sessions/sandboxes, default 16 |
| `AEX_GATEWAY_MAX_PENDING_SETUPS` | No | Slow/incomplete authentication and connect setup cap, default 256 |
| `AEX_GATEWAY_MAX_RELAY_BYTES` | No | Total bidirectional bytes per tunnel, default 2 GiB; this keeps one 1 GiB storage transfer viable with protocol overhead |
| `AEX_GATEWAY_SETUP_TIMEOUT_MS` | No | Absolute protocol/auth/connect setup bound, default 10 seconds |
| `AEX_GATEWAY_IDLE_TIMEOUT_MS` | No | Per-direction relay idle bound, default 5 minutes |

`GET /healthz` returns `200` on the independent health listener (and remains available on the proxy
listener for diagnostics); every other non-proxy HTTP route returns `404`. The service needs no
AWS credential or network route to Brain. Platform injects only the KMS public key and places the
listener behind a private NLB.

## Capability and proxy use

Environments constructs `Capability`, calls `unsigned_capability_bytes`, SHA-256 hashes those exact bytes,
and calls KMS `Sign` with `MessageType=DIGEST` and `ECDSA_SHA_256`. It then calls
`encode_signed_token` with KMS's DER signature. The payload binds root, session, sandbox, generation,
issue/expiry times, Brain's sealed policy digest and canonical destinations. It expires no later
than the eight-hour MicroVM wall. Canonical JSON is limited to 7,607 bytes and the final signed
token to 10,240 bytes. That same encoded-token bound is enforced for both Bearer and Basic auth;
even Basic's second base64 expansion leaves at least 2 KiB for the CONNECT request line and normal
headers inside the 16 KiB whole-header limit. An oversized policy fails before a capability is
injected into a sandbox.

Host grants are TLS/443 only. After CONNECT succeeds, the gateway buffers at most 64 KiB for one
ClientHello and requires exact visible SNI; missing/mismatched SNI and ECH fail closed. Explicit
IPv4 CIDR grants may carry raw TCP on declared ports. IPv6, UDP and QUIC are unsupported.

HTTP clients send:

```text
CONNECT datasets.example.com:443 HTTP/1.1
Proxy-Authorization: Bearer <capability>
```

For package managers that source proxy credentials from `HTTPS_PROXY`, the gateway also accepts
standard Basic credentials with literal username `aex` and the capability as password. The Environment can
therefore inject `http://aex:<capability>@<private-gateway-authority>` without a custom guest proxy.

SOCKS is deliberately unsupported: RFC 1929's 510-byte authentication ceiling cannot carry even a
normal one-destination Aex capability with production-sized identities. HTTP CONNECT is the sole
MVP proxy protocol.

## Image

CI publishes an immutable multi-architecture image only after the repository suite passes:

```text
ghcr.io/aexhq/environment-egress-gateway:sha-<40-character-environments-commit>
```

The workflow verifies both architecture children, bootstraps or verifies public GHCR visibility,
and proves the resolved manifest can be inspected without registry credentials before it emits a
deployable identity. If the repository token cannot change first-package visibility, it fails with
an explicit package-administrator action instead of publishing a false release success. Deployments
consume the exact digest:

```text
ghcr.io/aexhq/environment-egress-gateway@sha256:<64-hex-manifest-digest>
```

There is deliberately no `latest` deployment tag. Build locally with:

```sh
docker build -f gateway/Dockerfile -t environment-egress-gateway:dev .
```
