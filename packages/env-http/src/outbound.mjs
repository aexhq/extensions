import { request } from "node:https";
import { lookup } from "node:dns";
import { BlockList, isIP } from "node:net";

const denied = new BlockList();
for (const [address, prefix] of [["0.0.0.0", 8], ["10.0.0.0", 8], ["100.64.0.0", 10], ["127.0.0.0", 8],
  ["169.254.0.0", 16], ["172.16.0.0", 12], ["192.0.0.0", 24], ["192.0.2.0", 24], ["192.168.0.0", 16],
  ["198.18.0.0", 15], ["198.51.100.0", 24], ["203.0.113.0", 24], ["224.0.0.0", 3]]) denied.addSubnet(address, prefix);
const globalV6 = new BlockList();
globalV6.addSubnet("2000::", 3, "ipv6");
denied.addSubnet("2001::", 23, "ipv6");
denied.addSubnet("2002::", 16, "ipv6");
denied.addSubnet("2001:db8::", 32, "ipv6");

export function publicAddress(address) {
  const family = isIP(address);
  return family === 4 ? !denied.check(address) : family === 6
    && globalV6.check(address, "ipv6") && !denied.check(address, "ipv6");
}

export function publicJson(url, { token, body, signal, maxBytes = 4_194_304 }) {
  const destination = new URL(url);
  if (destination.protocol !== "https:" || destination.username || destination.password || destination.hash
    || destination.search || (destination.port && destination.port !== "443")) throw new TypeError("destination must be an HTTPS endpoint on port 443");
  const hostname = destination.hostname.replace(/^\[|\]$/gu, "");
  if (isIP(hostname) && !publicAddress(hostname)) throw new TypeError("destination is not public");
  return new Promise((resolve, reject) => {
    const req = request(destination, { method: "POST", signal, agent: false,
      headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
      lookup: (host, options, callback) => lookup(host, { all: true }, (error, addresses) => {
        if (error) return callback(error);
        if (!addresses.length || addresses.some(({ address }) => !publicAddress(address))) return callback(new Error("destination resolves to a non-public address"));
        // Resolve once inside socket creation, so the checked address is the connected address.
        if (options.all) callback(null, addresses);
        else callback(null, addresses[0].address, addresses[0].family);
      }),
    }, res => {
      if (res.statusCode !== 200) { res.destroy(); reject(new Error(`application returned HTTP ${res.statusCode}`)); return; }
      const chunks = [];
      let size = 0;
      res.on("data", chunk => {
        size += chunk.length;
        if (size > maxBytes) res.destroy(new Error("application response exceeds byte limit"));
        else chunks.push(chunk);
      });
      res.on("error", reject);
      res.on("end", () => { try { resolve(JSON.parse(Buffer.concat(chunks).toString("utf8"))); } catch (error) { reject(error); } });
    });
    req.on("error", reject);
    req.end(JSON.stringify(body));
  });
}
