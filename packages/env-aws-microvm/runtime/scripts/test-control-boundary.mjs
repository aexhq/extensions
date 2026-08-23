import assert from "node:assert/strict";
import http from "node:http";
import { networkInterfaces } from "node:os";
import net from "node:net";

const port = 8080;
assert.notEqual(process.geteuid?.(), 0);
assert.notEqual(process.geteuid?.(), 1001);
assert.throws(() => process.setuid(1001));

if (process.argv.includes("--bind")) {
  for (const host of ["0.0.0.0", "::"]) {
    const result = await new Promise((resolve) => {
      const server = net.createServer();
      server.once("error", (error) => resolve(error));
      server.listen(port, host, () => {
        server.close();
        resolve(new Error(`unprivileged Tool unexpectedly bound ${host}:${port}`));
      });
    });
    if (host === "::" && result.code === "EAFNOSUPPORT") continue;
    assert.equal(result.code, "EACCES");
  }
  process.exit(0);
}

const destinations = new Set(["127.0.0.1"]);
for (const addresses of Object.values(networkInterfaces())) {
  for (const address of addresses ?? []) {
    if (address.family === "IPv4") destinations.add(address.address);
  }
}

async function controlIsUnauthorized(host) {
  return new Promise((resolve, reject) => {
    const request = http.get({ host, port, path: "/" });
    const timer = setTimeout(() => {
      request.destroy();
      reject(new Error(`Tool control probe timed out at ${host}:${port}`));
    }, 1_000);
    request.once("response", (response) => {
      clearTimeout(timer);
      response.resume();
      resolve(response.statusCode === 401);
    });
    request.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

for (const destination of destinations) {
  assert.equal(await controlIsUnauthorized(destination), true);
}
