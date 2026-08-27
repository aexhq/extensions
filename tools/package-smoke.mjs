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
  runNpm(["install", "--no-package-lock", "--no-audit", "--no-fund", ...packages], { cwd: consumer });
  await writeFile(path.join(consumer, "smoke.mjs"), `import assert from "node:assert/strict";
import { BrainClient } from "@aexhq/brain";
import { defineAgentloop } from "@aexhq/agentloop";
import { createEnvironment } from "@aexhq/env-app";
import { awsMicrovm } from "@aexhq/env-aws-microvm";
import { packageUrl as codexPackage } from "@aexhq/loop-codex";
import { packageUrl as piPackage } from "@aexhq/loop-pi";
import { definitions, handlers } from "@aexhq/tools";

assert.equal(typeof new BrainClient({ baseUrl: "http://127.0.0.1:8080" }).listSessions, "function");
assert.equal(typeof defineAgentloop({ step: (input) => ({ context: input.context, decision: { type: "finish" } }) }).step, "function");
assert.equal(typeof createEnvironment({ tools: handlers }), "function");
assert.equal(awsMicrovm({ id: "smoke" }).environment_id, "smoke");
assert.equal(codexPackage.protocol, "file:");
assert.equal(piPackage.protocol, "file:");
assert.equal(definitions.bash.remoteToolId, "bash");
console.log("packed extension packages compose through the public Brain contracts");
`);
  const output = run(process.execPath, ["smoke.mjs"], { cwd: consumer });
  assert.match(output, /public Brain contracts/u);
  process.stdout.write(`${output}\n`);
} finally {
  await rm(temporary, { recursive: true, force: true });
}
