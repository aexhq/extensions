import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const npmCli = process.env.npm_execpath;
if (npmCli === undefined) throw new Error("run package-smoke through npm so its CLI is discoverable");
const temporary = await mkdtemp(path.join(tmpdir(), "extensions-package-smoke-"));
const artifacts = path.join(temporary, "artifacts");
const consumer = path.join(temporary, "consumer");
const run = (command, args, options = {}) =>
  execFileSync(command, args, { encoding: "utf8", stdio: "pipe", ...options }).trim();
const runNpm = (args, options = {}) => run(process.execPath, [npmCli, ...args], options);
const pack = (directory) => {
  const filename = runNpm(["pack", "--silent", "--pack-destination", artifacts], { cwd: directory })
    .split(/\r?\n/u).at(-1);
  if (filename === undefined || !filename.endsWith(".tgz")) throw new Error(`npm pack returned no archive for ${directory}`);
  return path.join(artifacts, filename);
};

try {
  await mkdir(artifacts);
  await mkdir(consumer);
  const packages = ["loop-codex", "loop-pi", "tools", "env-local", "tools-mcp", "env-browser"]
    .map((name) => pack(path.join(root, "packages", name)));
  if (process.env.BRAIN_PACKAGE_ARCHIVE !== undefined) packages.unshift(path.resolve(process.env.BRAIN_PACKAGE_ARCHIVE));
  await writeFile(path.join(consumer, "package.json"), `${JSON.stringify({
    name: "extensions-clean-consumer", private: true, type: "module",
  }, null, 2)}\n`);
  runNpm(["install", "--no-audit", "--no-fund", ...packages, "typescript@5.9.2", "@types/node@24.3.0"], { cwd: consumer });
  await writeFile(path.join(consumer, "smoke.mjs"), `import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import {
  Brain, brainEnv, environment, inspectAgentloop, inspectTool, hostEnv, tool,
} from "@aexhq/brain";
import { codex } from "@aexhq/agentloop-codex";
import { pi } from "@aexhq/agentloop-pi";
import { read } from "@aexhq/tools";
import { local } from "@aexhq/env-local";
import { createLocalEnvironment } from "@aexhq/env-local/server";
import { browser, browserTools } from "@aexhq/env-browser";
import { createBrowserEnvironment } from "@aexhq/env-browser/server";
import { connectMcp, mcpTools } from "@aexhq/tools-mcp";
import { z } from "zod";

assert.equal(typeof new Brain({ baseUrl: "http://127.0.0.1:8080" }).sessions.create, "function");
assert.equal(typeof createLocalEnvironment, "function");
assert.equal(typeof createBrowserEnvironment, "function");
assert.equal(typeof connectMcp, "function");
assert.equal(typeof mcpTools, "function");
const localEnv = local({ name: "local", url: "https://local.example", token: "fixture", profile: "coding" });
assert.equal(inspectTool(read({ env: localEnv })).environment, localEnv);
const browserEnv = browser({ name: "browser", url: "https://browser.example", token: "fixture", profile: "web" });
assert.equal(browserTools({ env: browserEnv }).length, 5);
const resident = tool({
  name: "create_invoice",
  description: "Create an invoice.",
  input: z.object({}),
  run: async (_input, context) => {
    await context.emit("invoice_created", {});
    return {};
  },
})({ env: hostEnv({ name: "app" }) });
assert.equal(inspectTool(resident)?.definition.name, "create_invoice");
const loopRuntime = brainEnv({ name: "brain" });
const workspace = environment({ url: () => "https://environment.example" })({ name: "workspace" });
const readSource = inspectTool(read({ env: workspace }));
assert.equal(readSource.environment, workspace);
assert.deepEqual(readSource.implementation, { type: "aex_official_tool", version: 1, name: "read" });
assert.equal(inspectAgentloop(codex({ env: loopRuntime })).environment, loopRuntime);
assert.equal(inspectAgentloop(pi({ env: loopRuntime })).environment, loopRuntime);
const toolsPackage = new URL(import.meta.resolve("@aexhq/tools/package.json"));
const registry = JSON.parse(await readFile(new URL("./dist/runtime/registry.json", toolsPackage), "utf8"));
const todoRuntime = (await import(new URL("./dist/runtime/todo.mjs", toolsPackage))).default;
assert.equal(todoRuntime.contractDigest, registry.todo.contract_digest);
assert.deepEqual(registry.read.manifest.implementation, readSource.implementation);
assert.deepEqual(
  await todoRuntime.execute(
    { action: "set", items: [{ text: "packed", done: false }] },
    { signal: AbortSignal.timeout(1_000), workspace: process.cwd() },
  ),
  { items: [{ text: "packed", done: false }] },
);
console.log("packed extension packages compose through the public Brain contracts");
`);
  const output = run(process.execPath, ["smoke.mjs"], { cwd: consumer });
  assert.match(output, /public Brain contracts/u);
  await writeFile(path.join(consumer, "smoke.ts"), `import { Brain, brainEnv, environment } from "@aexhq/brain";
import { pi } from "@aexhq/agentloop-pi";
import { bash, read, write } from "@aexhq/tools";
import { local } from "@aexhq/env-local";
import { createLocalEnvironment } from "@aexhq/env-local/server";
import { browser, browserTools } from "@aexhq/env-browser";
import { createBrowserEnvironment } from "@aexhq/env-browser/server";
import { connectMcp, mcpTools } from "@aexhq/tools-mcp";
import { chromium } from "playwright";

void createLocalEnvironment({ directory: "/tmp/local", profiles: { coding: { image: "fixture", workspace: "write" } } });
void createBrowserEnvironment({ profiles: { web: () => chromium.launch({ chromiumSandbox: true }) } });
void local({ name: "local", url: "https://local.example", token: "fixture", profile: "coding" });
void browserTools({ env: browser({ name: "browser", url: "https://browser.example", token: "fixture", profile: "web" }) });
void connectMcp;
void mcpTools;

const brain = new Brain({ baseUrl: "http://127.0.0.1:8080" });
const loopRuntime = brainEnv({ name: "brain" });
const workspace = environment({ url: () => "https://environment.example" })({ name: "workspace" });
void brain.sessions.create({
  model: { provider: "vercel-ai-gateway", name: "openai/gpt-5-mini", apiKey: "test-key" },
  agentloop: pi({ env: loopRuntime }),
  tools: [read({ env: workspace }), write({ env: workspace }), bash({ env: workspace })],
});
`);
  runNpm(["exec", "--", "tsc", "--noEmit", "--strict", "--target", "ES2023", "--module", "NodeNext", "--moduleResolution", "NodeNext", "smoke.ts"], { cwd: consumer });
  process.stdout.write(`${output}\n`);
} finally {
  await rm(temporary, { recursive: true, force: true });
}
