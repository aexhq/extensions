import https from "node:https";
import net from "node:net";

const denied = __DENIED__;
const controls = __CONTROLS__;
const httpSurfaces = __HTTP_SURFACES__;
const customerEnvironmentHosts = __CUSTOMER_ENVIRONMENT_HOSTS__;

__PROBE__

async function anyReachable(host, ports) {
  const outcomes = await Promise.all(ports.map((port) => probe(host, port)));
  return outcomes.some(Boolean);
}

function requestStatus(module, options) {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (status) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(status);
    };
    const request = module.request(options);
    const timer = setTimeout(() => {
      request.destroy();
      finish(null);
    }, 3000);
    request.once("response", (response) => {
      response.resume();
      finish(response.statusCode ?? null);
    });
    request.once("upgrade", (response, socket) => {
      socket.destroy();
      finish(response.statusCode ?? 101);
    });
    request.once("error", () => finish(null));
    request.end();
  });
}

function requestText(module, options, maxBytes) {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(value);
    };
    const request = module.request(options);
    const timer = setTimeout(() => {
      request.destroy();
      finish(null);
    }, 3000);
    request.once("response", (response) => {
      if (response.statusCode !== 200) {
        response.resume();
        finish(null);
        return;
      }
      const chunks = [];
      let bytes = 0;
      response.on("data", (chunk) => {
        bytes += chunk.length;
        if (bytes > maxBytes) {
          response.destroy();
          finish(null);
          return;
        }
        chunks.push(chunk);
      });
      response.once("end", () => finish(Buffer.concat(chunks).toString("utf8").trim()));
      response.once("error", () => finish(null));
    });
    request.once("error", () => finish(null));
    request.end();
  });
}

const specialResults = await Promise.all(
  denied.map(async (host) => [host, await anyReachable(host, [80, 443])]),
);
const reachableSpecial = specialResults.filter(([, reachable]) => reachable).map(([host]) => host);
if (reachableSpecial.length !== 0) {
  throw new Error(`special-use destinations accepted TCP: ${reachableSpecial.join(",")}`);
}

const controlResults = await Promise.all(
  controls.map(async (host) => [host, await anyReachable(host, [53, 80, 443])]),
);
const reachableControls = controlResults.filter(([, reachable]) => reachable).map(([host]) => host);
if (reachableControls.length === 0) {
  throw new Error("no public control was reachable");
}

const rawPublicSource = await requestText(https, {
  hostname: "checkip.amazonaws.com",
  path: "/",
  port: 443,
  method: "GET",
}, 64);
const observedPublicSource = net.isIP(rawPublicSource ?? "") === 4
  ? rawPublicSource
  : "unavailable";

const aexSurfaceResults = await Promise.all(httpSurfaces.map(async (surface) => ({
  surface,
  status: await requestStatus(https, {
    hostname: surface.host,
    path: surface.path,
    port: 443,
    method: "GET",
  }),
})));
for (const { surface, status } of aexSurfaceResults) {
  if (status !== 403) {
    throw new Error(
      `Aex HTTPS surface did not return the expected source denial: ${surface.host} status=${status} source=${observedPublicSource}`,
    );
  }
}

const customerEnvironmentResults = await Promise.all(customerEnvironmentHosts.map(async (host) => ({
  host,
  websocketStatus: await requestStatus(https, {
    hostname: host,
    path: "/v1",
    port: 443,
    method: "GET",
    headers: {
      Connection: "Upgrade",
      Upgrade: "websocket",
      "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==",
      "Sec-WebSocket-Version": "13",
    },
  }),
  managementStatus: await requestStatus(https, {
    hostname: host,
    // Use the documented API Gateway connection-id shape so the request reaches IAM
    // authentication instead of failing earlier as an invalid identifier.
    path: "/v1/@connections/L0SM9cOFvHcCIhw%3D",
    port: 443,
    method: "POST",
    headers: { "Content-Length": "0" },
  }),
})));
for (const { host, websocketStatus, managementStatus } of customerEnvironmentResults) {
  if (websocketStatus !== 401 && websocketStatus !== 403) {
    throw new Error(`customer Environment WebSocket did not return an authentication denial: ${host}`);
  }
  if (managementStatus !== 401 && managementStatus !== 403) {
    throw new Error(`customer Environment Management API did not return an authentication denial: ${host}`);
  }
}

process.stdout.write(`network_canary=ok denied=${denied.length} controls=${controls.length} surfaces=${httpSurfaces.length + customerEnvironmentHosts.length} source=${observedPublicSource}\n`);
