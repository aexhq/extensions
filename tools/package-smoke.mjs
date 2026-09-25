import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { packageWorkspaces } from "./workspaces.mjs";

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
  const workspaces = await packageWorkspaces();
  const toolNames = workspaces.filter(name => name.startsWith("tool-")).map(name => name.slice("tool-".length));
  const packages = workspaces
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
import { env } from "@aexhq/env";
import { codex } from "@aexhq/agentloop-codex";
import { pi } from "@aexhq/agentloop-pi";
${toolNames.map(name => `import { ${name} } from "@aexhq/tool-${name}";`).join("\n")}
import { http, httpTool } from "@aexhq/env-http";
import { createToolHandler } from "@aexhq/env-http/handler";
import { createHttpEnvironment } from "@aexhq/env-http/server";
import { local } from "@aexhq/env-local";
import { createLocalEnvironment } from "@aexhq/env-local/server";
import { modal } from "@aexhq/env-modal";
import { createModalEnvironment } from "@aexhq/env-modal/server";
import { browser, browserTools } from "@aexhq/env-browser";
import { createBrowserEnvironment } from "@aexhq/env-browser/server";
import { connectMcp, mcpTools } from "@aexhq/tools-mcp";
import { z } from "zod";

assert.equal(typeof new Brain({ baseUrl: "http://127.0.0.1:8080" }).sessions.create, "function");
assert.equal(inspectTool(env()).definition.name, "env");
assert.equal(typeof createToolHandler, "function");
assert.equal(typeof createHttpEnvironment, "function");
const httpEnv = http({ name: "app", url: "https://bridge.example", binding: "app-v1" });
const httpRead = tool({ name: "http_read", description: "Read", input: z.object({}), run: () => ({ status: "cancelled" }) });
assert.equal(inspectTool(httpTool(httpRead(), { env: httpEnv })).implementation.type, "http_tool");
assert.equal(typeof createLocalEnvironment, "function");
assert.equal(typeof createModalEnvironment, "function");
const modalEnv = modal({ name: "modal", url: "https://modal.example", token: "fixture", profile: "cpu", lifetimeMs: 300000 });
const calculate = tool({ name: "calculate", description: "Calculate", input: z.object({}), implementation: { type: "modal_command", name: "calculate" } });
assert.equal(inspectTool(calculate({ env: modalEnv })).environment, modalEnv);
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
assert.deepEqual(readSource.implementation, { type: "node_package", package: "@aexhq/tool-read", version: "8.1.0", entry: "./runtime", export: "read", configuration: {} });
assert.equal(inspectAgentloop(codex({ env: loopRuntime })).environment, loopRuntime);
assert.equal(inspectAgentloop(pi({ env: loopRuntime })).environment, loopRuntime);
for (const [name, factory] of Object.entries({ ${toolNames.join(", ")} })) {
  const toolPackage = new URL(import.meta.resolve("@aexhq/tool-" + name + "/package.json"));
  const document = JSON.parse(await readFile(toolPackage, "utf8"));
  assert.ok(!Object.keys(document.dependencies).some(dependency => dependency.startsWith("@aexhq/tool")));
  const registry = JSON.parse(await readFile(new URL("./dist/runtime/registry.json", toolPackage), "utf8"));
  assert.deepEqual(Object.keys(registry), [name]);
  const runtime = (await import(new URL("./dist/runtime/" + name + ".mjs", toolPackage))).default;
  assert.equal(runtime.contractDigest, registry[name].contract_digest);
  const source = inspectTool(factory({ env: workspace }));
  assert.deepEqual(registry[name].manifest, {
    name: source.definition.name, description: source.definition.description,
    input_schema: source.definition.inputSchema, output_schema: source.definition.outputSchema,
    implementation: { type: "aex_official_tool", version: 1, name },
  });
}
const todoPackage = new URL(import.meta.resolve("@aexhq/tool-todo/package.json"));
const todoRuntime = (await import(new URL("./dist/runtime/todo.mjs", todoPackage))).default;
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
  await writeFile(path.join(consumer, "smoke.ts"), `import { Brain, brainEnv, environment, hostEnv } from "@aexhq/brain";
import { pi } from "@aexhq/agentloop-pi";
${toolNames.map(name => `import { ${name} } from "@aexhq/tool-${name}";`).join("\n")}
import { http, httpTool } from "@aexhq/env-http";
import { createToolHandler } from "@aexhq/env-http/handler";
import { createHttpEnvironment } from "@aexhq/env-http/server";
import { local } from "@aexhq/env-local";
import { createLocalEnvironment } from "@aexhq/env-local/server";
import { browser, browserTools } from "@aexhq/env-browser";
import { createBrowserEnvironment } from "@aexhq/env-browser/server";
import { connectMcp, mcpTools } from "@aexhq/tools-mcp";
import { chromium } from "playwright";

void createToolHandler({ tools: [], authorize: async () => {} });
void createHttpEnvironment({ authorize: async () => ({ url: "https://app.example/tools", token: "secret", timeoutMs: 1000, tools: [] }) });
void httpTool;
void http({ name: "app", url: "https://bridge.example", binding: "app-v1" });
void createLocalEnvironment({ directory: "/tmp/local", profiles: { coding: { image: "fixture", workspace: "write" } } });
void createBrowserEnvironment({ profiles: { web: () => chromium.launch({ chromiumSandbox: true }) },
  publishMedia: async ({ bytes, mediaType, index }, { sessionId, sequence, signal }) => {
    signal.throwIfAborted();
    return "https://media.example/" + [sessionId, sequence, index, bytes.byteLength, mediaType].join("/");
  } });
void local({ name: "local", url: "https://local.example", token: "fixture", profile: "coding" });
void browserTools({ env: browser({ name: "browser", url: "https://browser.example", token: "fixture", profile: "web" }) });
void connectMcp;
declare const client: Parameters<typeof mcpTools>[0]["client"];
void mcpTools({ client, env: hostEnv({ name: "app" }), names: ["read_report"],
  publishMedia: async ({ bytes, mediaType, index }, { sessionId, sequence, signal }) => {
    signal.throwIfAborted();
    return "https://media.example/" + [sessionId, sequence, index, bytes.byteLength, mediaType].join("/");
  } });

const brain = new Brain({ baseUrl: "http://127.0.0.1:8080" });
const loopRuntime = brainEnv({ name: "brain" });
const workspace = environment({ url: () => "https://environment.example" })({ name: "workspace" });
void brain.sessions.create({
  environmentLifecycle: { default: "automatic" },
  model: { provider: "vercel-ai-gateway", name: "openai/gpt-5-mini", apiKey: "test-key" },
  agentloop: pi({ env: loopRuntime }),
  tools: [${toolNames.map(name => `${name}({ env: workspace })`).join(", ")}],
});
`);
  runNpm(["exec", "--", "tsc", "--noEmit", "--strict", "--target", "ES2023", "--module", "NodeNext", "--moduleResolution", "NodeNext", "smoke.ts"], { cwd: consumer });
  process.stdout.write(`${output}\n`);
} finally {
  await rm(temporary, { recursive: true, force: true });
}
