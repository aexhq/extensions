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
  const packages = ["env-aws-microvm", "loop-codex", "loop-pi", "tools"]
    .map((name) => pack(path.join(root, "packages", name)));
  if (process.env.BRAIN_PACKAGE_ARCHIVE !== undefined) packages.unshift(path.resolve(process.env.BRAIN_PACKAGE_ARCHIVE));
  await writeFile(path.join(consumer, "package.json"), `${JSON.stringify({
    name: "extensions-clean-consumer", private: true, type: "module",
  }, null, 2)}\n`);
  runNpm(["install", "--no-audit", "--no-fund", ...packages, "typescript@5.9.2", "@types/node@24.3.0"], { cwd: consumer });
  await writeFile(path.join(consumer, "smoke.mjs"), `import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import {
  Brain, brainWasm, inspectAgentloop, inspectEnvironment, inspectPlacedTool, inspectResidentTool, tool,
} from "@aexhq/brain";
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { codex } from "@aexhq/agentloop-codex";
import { pi } from "@aexhq/agentloop-pi";
import { read } from "@aexhq/tools";
import { z } from "zod";

assert.equal(typeof new Brain({ baseUrl: "http://127.0.0.1:8080" }).sessions.create, "function");
const resident = tool({
  name: "create_invoice",
  description: "Create an invoice.",
  input: z.object({}),
  run: async (_input, context) => {
    await context.emit("invoice_created", {});
    return {};
  },
})();
assert.equal(inspectResidentTool(resident)?.definition.name, "create_invoice");
const loopRuntime = brainWasm();
const workspace = awsMicroVm({ region: "eu-west-2" });
assert.equal(inspectEnvironment(workspace).configuration.driver, "aws-microvm");
const readSource = inspectPlacedTool(read({ env: workspace }));
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
  await writeFile(path.join(consumer, "smoke.ts"), `import { Brain, brainWasm } from "@aexhq/brain";
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/agentloop-pi";
import { bash, read, write } from "@aexhq/tools";

const brain = new Brain({ baseUrl: "http://127.0.0.1:8080" });
const loopRuntime = brainWasm();
const workspace = awsMicroVm({ region: "eu-west-2" });
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
