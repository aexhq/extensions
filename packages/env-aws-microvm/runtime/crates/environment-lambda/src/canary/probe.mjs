// Shared TCP reachability probe for the network canaries. Concatenated into each canary script
// by the Rust command builder; `net` is imported by the including script.
function probe(host, port) {
  return new Promise((resolve) => {
    let settled = false;
    const socket = net.connect({ host, port });
    const finish = (reachable) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.destroy();
      resolve(reachable);
    };
    const timer = setTimeout(() => finish(false), 1500);
    socket.once("connect", () => finish(true));
    socket.once("error", () => finish(false));
  });
}
