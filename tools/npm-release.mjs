import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const workspaces = ["agentloop", "env-app", "env-aws-microvm", "loop-codex", "loop-pi", "tools"];
const root = path.resolve(import.meta.dirname, "..");
const npmCli = [
  process.env.npm_execpath,
  path.join(path.dirname(process.execPath), "node_modules", "npm", "bin", "npm-cli.js"),
  path.resolve(path.dirname(process.execPath), "../lib/node_modules/npm/bin/npm-cli.js"),
].find((candidate) => candidate !== undefined && existsSync(candidate));
if (npmCli === undefined) throw new Error("could not locate npm-cli.js for the active Node runtime");

const run = (args) => execFileSync(process.execPath, [npmCli, ...args], {
  cwd: root,
  encoding: "utf8",
  stdio: ["ignore", "pipe", "pipe"],
}).trim();

const document = async (workspace) =>
  JSON.parse(await readFile(path.join(root, "packages", workspace, "package.json"), "utf8"));

const packResult = (output, name) => {
  for (let start = output.lastIndexOf("\n["); start >= -1; start = output.lastIndexOf("\n[", start - 1)) {
    const candidate = output.slice(start + 1).trim();
    try {
      const parsed = JSON.parse(candidate);
      if (Array.isArray(parsed)) return parsed;
    } catch {}
    if (start === -1) break;
  }
  throw new Error(`npm pack returned no JSON result for ${name}`);
};

const manifest = async (filename) => {
  const value = JSON.parse(await readFile(filename, "utf8"));
  if (value.schema !== 1 || !Array.isArray(value.packages)) {
    throw new Error("release manifest has an unsupported shape");
  }
  return value;
};

async function pack(directory) {
  await mkdir(directory, { recursive: false });
  const packages = [];
  for (const workspace of workspaces) {
    const packageDocument = await document(workspace);
    const result = packResult(run([
      "pack", "--silent", "--json", "--workspace", packageDocument.name,
      "--pack-destination", directory,
    ]), packageDocument.name);
    if (!Array.isArray(result) || result.length !== 1) {
      throw new Error(`npm pack returned an unexpected result for ${packageDocument.name}`);
    }
    const item = result[0];
    if (item.name !== packageDocument.name || item.version !== packageDocument.version) {
      throw new Error(`packed identity drifted for ${packageDocument.name}`);
    }
    const archive = path.join(directory, item.filename);
    const integrity = `sha512-${createHash("sha512").update(await readFile(archive)).digest("base64")}`;
    if (integrity !== item.integrity) {
      throw new Error(`npm reported the wrong integrity for ${packageDocument.name}`);
    }
    packages.push({
      workspace,
      name: packageDocument.name,
      version: packageDocument.version,
      filename: item.filename,
      integrity,
      dependencies: packageDocument.dependencies ?? {},
      peerDependencies: packageDocument.peerDependencies ?? {},
    });
  }
  for (const item of packages) {
    for (const [name, version] of Object.entries(item.dependencies)) {
      const local = packages.find((candidate) => candidate.name === name);
      if (local !== undefined && version !== local.version) {
        throw new Error(`${item.name} must depend on the exact release version ${name}@${local.version}`);
      }
    }
  }
  const value = { schema: 1, source: process.env.GITHUB_SHA ?? "local", packages };
  await writeFile(path.join(directory, "manifest.json"), `${JSON.stringify(value, null, 2)}\n`);
  for (const filename of ["npm-release.mjs", "verify-dependencies.mjs", "publish.mjs"]) {
    await writeFile(path.join(directory, filename), await readFile(path.join(root, "tools", filename)));
  }
}

const [command, argument] = process.argv.slice(2);
if (command === "pack" && argument !== undefined) await pack(path.resolve(argument));
else if (command === "versions" && argument !== undefined) {
  const value = await manifest(path.resolve(argument));
  process.stdout.write(value.packages.map(({ name, version }) => `${name}@${version}`).join(","));
} else if (command === "markdown" && argument !== undefined) {
  const value = await manifest(path.resolve(argument));
  process.stdout.write("| package | version | sha512 integrity |\n| --- | --- | --- |\n");
  for (const item of value.packages) {
    process.stdout.write(`| \`${item.name}\` | \`${item.version}\` | \`${item.integrity}\` |\n`);
  }
} else {
  throw new Error("usage: npm-release.mjs pack <directory> | versions|markdown <manifest.json>");
}
