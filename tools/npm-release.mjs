import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const workspaces = [
  "env-aws-microvm",
  "loop-codex",
  "loop-pi",
  "tools",
];
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

const git = (args) => execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();

const releasedIntegrity = (spec) => {
  try {
    const output = run(["view", spec, "dist.integrity", "--json"]);
    return output === "" ? undefined : JSON.parse(output);
  } catch {
    return undefined;
  }
};

export const releasedSourceCommit = (document) => {
  const provenance = document.attestations?.find(
    ({ predicateType }) => predicateType === "https://slsa.dev/provenance/v1",
  );
  const payload = provenance?.bundle?.dsseEnvelope?.payload;
  if (typeof payload !== "string") throw new Error("published package has no SLSA provenance");
  const statement = JSON.parse(Buffer.from(payload, "base64").toString("utf8"));
  const dependency = statement.predicate?.buildDefinition?.resolvedDependencies?.find(
    ({ uri }) => uri?.startsWith("git+https://github.com/aexhq/extensions@"),
  );
  const commit = dependency?.digest?.gitCommit;
  if (typeof commit !== "string" || !/^[0-9a-f]{40}$/u.test(commit)) {
    throw new Error("published package provenance has no Extensions source commit");
  }
  return commit;
};

const releasedAttestations = async (spec) => {
  const output = run(["view", spec, "dist.attestations.url", "--json"]);
  const url = JSON.parse(output);
  if (typeof url !== "string" || !url.startsWith("https://registry.npmjs.org/-/npm/v1/attestations/")) {
    throw new Error(`${spec} has no registry attestations`);
  }
  const response = await fetch(url);
  if (!response.ok) throw new Error(`could not read ${spec} provenance: HTTP ${response.status}`);
  return response.json();
};

/**
 * Rebuilding a component is not byte-for-byte deterministic. The registry provenance identifies
 * the source that produced the immutable package, so compare source rather than rebuilt archives.
 */
const assertReleasedVersionIsCurrent = async (workspace, spec) => {
  const directory = `packages/${workspace}`;
  const commit = releasedSourceCommit(await releasedAttestations(spec));
  git(["merge-base", "--is-ancestor", commit, "HEAD"]);
  try {
    git(["diff", "--quiet", `${commit}..HEAD`, "--", directory]);
  } catch {
    throw new Error(`${directory} changed after ${spec} was published; release it under a new version`);
  }
};

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
    if (packageDocument.publishConfig?.access !== "public" || packageDocument.publishConfig?.tag !== "next") {
      throw new Error(`${packageDocument.name} must publish publicly under the next dist-tag`);
    }
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
    // An exact version already on the registry is the released object. Keep it, prove the package
    // has not changed since, and drop the unusable rebuild so nothing can publish it.
    const spec = `${packageDocument.name}@${packageDocument.version}`;
    const released = releasedIntegrity(spec);
    if (released !== undefined) {
      await assertReleasedVersionIsCurrent(workspace, spec);
      await rm(archive);
    }
    packages.push({
      workspace,
      name: packageDocument.name,
      version: packageDocument.version,
      filename: item.filename,
      integrity: released ?? integrity,
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

const [command, argument] = process.argv[1] !== undefined &&
    path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
  ? process.argv.slice(2)
  : ["import"];
if (command === "import") { /* imported for its exported contracts */ }
else if (command === "pack" && argument !== undefined) await pack(path.resolve(argument));
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
