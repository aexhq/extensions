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
  if (filename === undefined || !filename.endsWith(".tgz")) {
    throw new Error(`npm pack returned no archive for ${directory}`);
  }
  return path.join(artifacts, filename);
};

try {
  await mkdir(artifacts);
  await mkdir(consumer);
  const packages = [
    "agentloop",
    "env-app",
    "env-aws-microvm",
    "loop-codex",
    "loop-pi",
    "model",
    "model-anthropic",
    "model-openai",
    "tools",
  ].map((name) => pack(path.join(root, "packages", name)));
  await writeFile(path.join(consumer, "package.json"), `${JSON.stringify({
    name: "extensions-clean-consumer", private: true, type: "module",
  }, null, 2)}\n`);
  runNpm(["install", "--no-package-lock", "--no-audit", "--no-fund", ...packages], { cwd: consumer });
  await writeFile(path.join(consumer, "smoke.mjs"), `import assert from "node:assert/strict";
import { prepareComponent } from "@aexhq/brain";
import { app } from "@aexhq/env-app";
import { awsMicrovm } from "@aexhq/env-aws-microvm";
import { codex } from "@aexhq/loop-codex";
import { pi } from "@aexhq/loop-pi";
import { anthropic } from "@aexhq/model-anthropic";
import { openai } from "@aexhq/model-openai";
import { bash, subagents } from "@aexhq/tools";

const values = [app({ id: "smoke" }), awsMicrovm(), pi(), codex(), anthropic(), openai(), bash(), subagents()];
assert.deepEqual(values.map((value) => value.extension), [
  "environment", "environment", "agentloop", "agentloop", "model", "model", "tool", "tool",
]);
for (const value of values) assert.ok((await prepareComponent(value)).bytes > 0);
console.log("packed extension packages compose through the public Brain SDK");
`);
  const output = run(process.execPath, ["smoke.mjs"], { cwd: consumer });
  assert.match(output, /public Brain SDK/u);
  process.stdout.write(`${output}\n`);
} finally {
  await rm(temporary, { recursive: true, force: true });
}
