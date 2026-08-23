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
    pack(path.join(root, "../aex/packages/contracts")),
    pack(path.join(root, "../aex/packages/environment")),
    pack(path.join(root, "../aex/packages/session-protocol")),
    pack(path.join(root, "../aex/packages/sdk")),
    ...["agentloop", "env-app", "env-aws-microvm", "loop-codex", "loop-pi", "tools"]
      .map((name) => pack(path.join(root, "packages", name))),
  ];
  await writeFile(path.join(consumer, "package.json"), `${JSON.stringify({
    name: "extensions-clean-consumer", private: true, type: "module",
  }, null, 2)}\n`);
  runNpm(["install", "--no-package-lock", "--no-audit", "--no-fund", ...packages], { cwd: consumer });
  await writeFile(path.join(consumer, "smoke.mjs"), `import assert from "node:assert/strict";
import { inspectEnvironment } from "@aexhq/environment";
import { app } from "@aexhq/env-app";
import { awsMicrovm } from "@aexhq/env-aws-microvm";
import { codex } from "@aexhq/loop-codex";
import { pi } from "@aexhq/loop-pi";
import { bash, subagents } from "@aexhq/tools";

const application = app({ id: "smoke" });
const environment = awsMicrovm();
assert.equal(inspectEnvironment(application).serialized.profile.kind, "callbacks");
assert.equal(inspectEnvironment(environment).serialized.profile.kind, "computer");
assert.match(pi().sha256, /^[0-9a-f]{64}$/u);
assert.match(codex().sha256, /^[0-9a-f]{64}$/u);
assert.equal(bash().kind, "aex.tool");
assert.equal(subagents().kind, "aex.tool");
console.log("packed extension packages compose through public Aex contracts");
`);
  const output = run(process.execPath, ["smoke.mjs"], { cwd: consumer });
  assert.match(output, /compose through public Aex contracts/u);
  process.stdout.write(`${output}\n`);
} finally {
  await rm(temporary, { recursive: true, force: true });
}
