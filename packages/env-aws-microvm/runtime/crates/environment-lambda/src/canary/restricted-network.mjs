import dns from "node:dns";
import net from "node:net";

const connectorClass = __CLASS__;
const denied = __DENIED__;
const controls = __CONTROLS__;
const gateway = __GATEWAY__;
const requireGateway = __REQUIRE_GATEWAY__;

__PROBE__

function dnsOutcome(host) {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (outcome) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(outcome);
    };
    const timer = setTimeout(() => finish("timeout"), 3000);
    dns.lookup(host, (error) => finish(error ? "blocked" : "resolved"));
  });
}

function gatewayStatus(request) {
  return new Promise((resolve) => {
    let settled = false;
    let response = "";
    const socket = net.connect({ host: gateway.host, port: gateway.port });
    const finish = (status) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.destroy();
      resolve(status);
    };
    const timer = setTimeout(() => finish(null), 3000);
    socket.once("connect", () => socket.write(request));
    socket.on("data", (chunk) => {
      response += chunk.toString("ascii");
      const lineEnd = response.indexOf("\r\n");
      if (lineEnd === -1) return;
      const match = /^HTTP\/1\.1 ([0-9]{3}) /.exec(response.slice(0, lineEnd));
      finish(match ? Number(match[1]) : null);
    });
    socket.once("error", () => finish(null));
    socket.once("end", () => finish(null));
  });
}

const dnsState = await dnsOutcome("example.com");
if (dnsState !== "blocked") {
  throw new Error(`restricted connector DNS was not fail-closed: ${dnsState}`);
}

const directHosts = [...new Set([...denied, ...controls])];
if (!requireGateway) directHosts.push(gateway.host);
const directResults = await Promise.all(directHosts.map(async (host) => [
  host,
  (await Promise.all([53, 80, 443, 8443].map((port) => probe(host, port)))).some(Boolean),
]));
const reachableDirect = directResults.filter(([, reachable]) => reachable).map(([host]) => host);
if (reachableDirect.length !== 0) {
  throw new Error(`restricted connector accepted direct TCP: ${reachableDirect.join(",")}`);
}

if (requireGateway) {
  const health = await gatewayStatus(
    `GET /healthz HTTP/1.1\r\nHost: ${gateway.host}\r\nConnection: close\r\n\r\n`,
  );
  if (health !== 200) throw new Error(`allowlist gateway health was not reachable: ${health}`);
  const unauthenticated = await gatewayStatus(
    "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nConnection: close\r\n\r\n",
  );
  if (unauthenticated !== 407) {
    throw new Error(`allowlist gateway accepted or misclassified missing auth: ${unauthenticated}`);
  }
  const invalid = await gatewayStatus(
    "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nProxy-Authorization: Bearer invalid-release-canary-capability\r\nConnection: close\r\n\r\n",
  );
  if (invalid !== 403) {
    throw new Error(`allowlist gateway accepted or misclassified invalid auth: ${invalid}`);
  }
}

process.stdout.write(`restricted_network_canary=ok class=${connectorClass} denied=${denied.length} controls=${controls.length}\n`);
