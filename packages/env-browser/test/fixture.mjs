import { createServer } from "node:http";
import { once } from "node:events";
import { chromium } from "playwright";
import { createBrowserEnvironment } from "../dist/server.mjs";

export async function browserFixture(t) {
  const server = createServer((request, response) => {
    if (request.url === "/hanging") { response.writeHead(200, { "content-type": "text/html" }); response.write("<!doctype html><html>"); return; }
    response.end(`<!doctype html><title>Browser fixture</title>
    <label>Name <input id="name"></label><button id="save" onclick="localStorage.setItem('name',document.querySelector('#name').value);document.querySelector('#status').textContent=localStorage.getItem('name')">Save</button>
    <p id="status">${request.url === "/second" ? "second page" : "ready"}</p>`);
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const launched = [];
  const env = createBrowserEnvironment({ profiles: { test: async () => {
    const browser = await chromium.launch({ chromiumSandbox: true });
    launched.push(browser);
    return browser;
  } } });
  t.after(async () => { await env.close(); await new Promise(resolve => server.close(resolve)); });
  return { env, launched, url: `http://127.0.0.1:${server.address().port}` };
}
