import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const npmCli = [
  process.env.npm_execpath,
  path.join(path.dirname(process.execPath), "node_modules", "npm", "bin", "npm-cli.js"),
  path.resolve(path.dirname(process.execPath), "../lib/node_modules/npm/bin/npm-cli.js"),
].find((candidate) => candidate !== undefined && existsSync(candidate));
if (npmCli === undefined) throw new Error("could not locate npm-cli.js for the active Node runtime");
const manifestPath = process.argv[2] === undefined
  ? path.join(import.meta.dirname, "manifest.json")
  : path.resolve(process.argv[2]);
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
const releaseNames = new Set(manifest.packages.map(({ name }) => name));
const required = new Map();
for (const item of manifest.packages) {
  for (const [name, version] of Object.entries(item.dependencies)) {
    if (name.startsWith("@aexhq/") && !releaseNames.has(name)) required.set(name, version);
  }
}
for (const [name, version] of required) {
  const spec = `${name}@${version}`;
  const actual = JSON.parse(execFileSync(
    process.execPath,
    [npmCli, "view", spec, "version", "dist.integrity", "--json"],
    { encoding: "utf8" },
  ));
  if (actual.version !== version) {
    throw new Error(`${spec} resolved to ${actual.version}; extensions require the exact version`);
  }
  if (typeof actual["dist.integrity"] !== "string" || !actual["dist.integrity"].startsWith("sha512-")) {
    throw new Error(`${spec} has no sha512 integrity on the public registry`);
  }
}
