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
const directory = import.meta.dirname;
const manifestPath = process.env.RELEASE_MANIFEST ?? path.join(directory, "manifest.json");
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
if (!/^[0-9a-f]{40}$/u.test(process.env.EXPECTED_COMMIT ?? "") ||
    manifest.source !== process.env.EXPECTED_COMMIT) {
  throw new Error("the release archive source does not match EXPECTED_COMMIT");
}

const run = (args, stdio = "pipe") => {
  const output = execFileSync(process.execPath, [npmCli, ...args], { encoding: "utf8", stdio });
  return typeof output === "string" ? output.trim() : "";
};
const registryValue = (spec, field) => {
  try { return JSON.parse(run(["view", spec, field, "--json"])); } catch { return undefined; }
};
const registryTags = (name) => JSON.parse(run(["view", name, "dist-tags", "--json"]));
const waitFor = async (read, expected, description) => {
  for (let attempt = 0; attempt < 12; attempt += 1) {
    if (read() === expected) return;
    await new Promise((resolve) => setTimeout(resolve, 5_000));
  }
  throw new Error(`${description} did not become ${JSON.stringify(expected)} within 60 seconds`);
};
const assertRegistryObject = (item) => {
  const spec = `${item.name}@${item.version}`;
  const integrity = registryValue(spec, "dist.integrity");
  if (integrity !== item.integrity) {
    throw new Error(integrity === undefined
      ? `${spec} is not visible on the public registry`
      : `${spec} exists with integrity ${integrity}, not ${item.integrity}`);
  }
};

const operation = process.argv[2];
if (operation === "bootstrap") {
  if (!process.env.NODE_AUTH_TOKEN) {
    throw new Error("the protected npm-production environment has no NPM_DIST_TAG_TOKEN");
  }
  const missing = [];
  for (const item of manifest.packages) {
    const spec = `${item.name}@${item.version}`;
    const integrity = registryValue(spec, "dist.integrity");
    if (integrity === undefined) missing.push(item);
    else if (integrity !== item.integrity) {
      throw new Error(`${spec} is immutable and already has a different registry integrity`);
    }
  }
  if (missing.length === 0) {
    throw new Error("every exact package version in this release already exists; use stage");
  }
  for (const item of missing) {
    const spec = `${item.name}@${item.version}`;
    const previousLatest = registryValue(`${item.name}@latest`, "version");
    run([
      "publish",
      path.join(directory, item.filename),
      "--access",
      "public",
      "--tag",
      "next",
      "--provenance",
    ], "inherit");
    await waitFor(() => registryValue(`${item.name}@next`, "version"), item.version, `${item.name}@next`);
    if (previousLatest === undefined && registryTags(item.name).latest === item.version) {
      run(["dist-tag", "rm", item.name, "latest"], "inherit");
      await waitFor(() => registryTags(item.name).latest, undefined, `${item.name}@latest removal`);
    }
    process.stdout.write(`bootstrapped ${spec} (${item.integrity})\n`);
  }
} else if (operation === "hold") {
  if (!process.env.NODE_AUTH_TOKEN) throw new Error("NPM_DIST_TAG_TOKEN is unavailable");
  for (const item of manifest.packages) {
    assertRegistryObject(item);
    const tags = registryTags(item.name);
    if (tags.next !== item.version) {
      throw new Error(`${item.name}@next is ${tags.next ?? "absent"}; refusing hold`);
    }
  }
  for (const item of manifest.packages) {
    if (registryTags(item.name).latest === item.version) {
      run(["dist-tag", "rm", item.name, "latest"], "inherit");
      await waitFor(() => registryTags(item.name).latest, undefined, `${item.name}@latest removal`);
      process.stdout.write(`removed automatic latest from ${item.name}@${item.version}\n`);
    }
  }
} else if (operation === "stage") {
  const existing = new Map();
  for (const item of manifest.packages) {
    const spec = `${item.name}@${item.version}`;
    const integrity = registryValue(spec, "dist.integrity");
    if (integrity !== undefined && integrity !== item.integrity) {
      throw new Error(`${spec} is immutable and already has a different registry integrity`);
    }
    existing.set(spec, integrity);
  }
  for (const item of manifest.packages) {
    const spec = `${item.name}@${item.version}`;
    if (existing.get(spec) === undefined) {
      run(["publish", path.join(directory, item.filename), "--access", "public", "--tag", "next", "--provenance"], "inherit");
      await waitFor(() => registryValue(spec, "dist.integrity"), item.integrity, `${spec} integrity`);
    }
    await waitFor(() => registryValue(`${item.name}@next`, "version"), item.version, `${item.name}@next`);
    process.stdout.write(`staged ${spec} (${item.integrity})\n`);
  }
} else if (operation === "promote") {
  if (!process.env.NODE_AUTH_TOKEN) throw new Error("NPM_DIST_TAG_TOKEN is unavailable");
  for (const item of manifest.packages) {
    assertRegistryObject(item);
    const staged = registryValue(`${item.name}@next`, "version");
    if (staged !== item.version) {
      throw new Error(`${item.name}@next is ${staged ?? "absent"}; refusing promotion`);
    }
  }
  for (const item of manifest.packages) {
    const spec = `${item.name}@${item.version}`;
    run(["dist-tag", "add", spec, "latest"], "inherit");
    await waitFor(() => registryValue(`${item.name}@latest`, "version"), item.version, `${item.name}@latest`);
    process.stdout.write(`promoted ${spec} without republishing\n`);
  }
} else {
  throw new Error("usage: publish.mjs bootstrap|stage|hold|promote");
}
