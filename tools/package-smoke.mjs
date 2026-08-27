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
  const packages = ["agentloop", "env-app", "env-aws-microvm", "loop-codex", "loop-pi", "tools"]
    .map((name) => pack(path.join(root, "packages", name)));
  if (process.env.BRAIN_PACKAGE_ARCHIVE !== undefined) packages.unshift(path.resolve(process.env.BRAIN_PACKAGE_ARCHIVE));
  await writeFile(path.join(consumer, "package.json"), `${JSON.stringify({
    name: "extensions-clean-consumer", private: true, type: "module",
  }, null, 2)}\n`);
  runNpm(["install", "--no-package-lock", "--no-audit", "--no-fund", ...packages, "typescript@5.9.2"], { cwd: consumer });
  await writeFile(path.join(consumer, "smoke.mjs"), `import assert from "node:assert/strict";
import { Brain } from "@aexhq/brain";
import { defineAgentloop } from "@aexhq/agentloop";
import { createEnvironment } from "@aexhq/env-app";
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { codex } from "@aexhq/loop-codex";
import { pi } from "@aexhq/loop-pi";
import { handlers, read } from "@aexhq/tools";

assert.equal(typeof new Brain({ baseUrl: "http://127.0.0.1:8080" }).createSession, "function");
assert.equal(typeof defineAgentloop({ step: (input) => ({ context: input.context, decision: { type: "finish" } }) }).step, "function");
assert.equal(typeof createEnvironment({ tools: handlers }), "function");
const workspace = awsMicroVm({ lifecycle: "shared", id: "smoke" });
assert.equal(read().runIn(workspace).kind, "bound-tool");
assert.equal(codex().kind, "agent-loop");
assert.equal(pi().kind, "agent-loop");
console.log("packed extension packages compose through the public Brain contracts");
`);
  const output = run(process.execPath, ["smoke.mjs"], { cwd: consumer });
  assert.match(output, /public Brain contracts/u);
  await writeFile(path.join(consumer, "smoke.ts"), `import { Brain } from "@aexhq/brain";
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/loop-pi";
import { bash, read, write } from "@aexhq/tools";

const brain = new Brain({ baseUrl: "http://127.0.0.1:8080" });
const workspace = awsMicroVm({ region: "eu-west-2" });
void brain.createSession({
  model: { provider: "vercel-ai-gateway", name: "openai/gpt-5-mini", apiKey: "test-key" },
  agentLoop: pi(),
  tools: [read().runIn(workspace), write().runIn(workspace), bash().runIn(workspace)],
});
`);
  runNpm(["exec", "--", "tsc", "--noEmit", "--strict", "--target", "ES2023", "--module", "NodeNext", "--moduleResolution", "NodeNext", "smoke.ts"], { cwd: consumer });
  process.stdout.write(`${output}\n`);
} finally {
  await rm(temporary, { recursive: true, force: true });
}
