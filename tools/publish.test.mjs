import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

const registry = {
  integrity: {
    "@aexhq/model@0.1.2": "sha512-model",
    "@aexhq/model-openai@0.1.3": "sha512-openai",
  },
  next: { "@aexhq/model": "0.1.2", "@aexhq/model-openai": "0.1.3" },
  latest: { "@aexhq/model": "0.0.0", "@aexhq/model-openai": "0.0.0" },
};

const manifest = {
  schema: 1,
  source: "0".repeat(40),
  packages: [
    {
      workspace: "model-openai",
      name: "@aexhq/model-openai",
      version: "0.1.3",
      integrity: "sha512-openai",
      needs: ["model"],
    },
    {
      workspace: "model",
      name: "@aexhq/model",
      version: "0.1.2",
      integrity: "sha512-model",
      needs: [],
    },
  ],
};

const npmStub = `
  import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
  const args = process.argv.slice(2);
  const state = JSON.parse(readFileSync(process.env.REGISTRY, "utf8"));
  const parse = (spec) => [spec.slice(0, spec.lastIndexOf("@")), spec.slice(spec.lastIndexOf("@") + 1)];
  if (args[0] === "dist-tag") {
    const [name, version] = parse(args[2]);
    appendFileSync(process.env.PROMOTED, args[2] + "\\n");
    state.latest[name] = version;
    writeFileSync(process.env.REGISTRY, JSON.stringify(state));
  } else {
    const [name, selector] = parse(args[1]);
    const value = args[2] === "dist.integrity"
      ? state.integrity[args[1]]
      : selector === "next" ? state.next[name] : state.latest[name];
    if (value === undefined) process.exitCode = 1;
    else process.stdout.write(JSON.stringify(value));
  }
`;

const promote = async (receipts) => {
  const directory = await mkdtemp(path.join(tmpdir(), "extensions-promote-"));
  const state = path.join(directory, "registry.json");
  const promoted = path.join(directory, "promoted.txt");
  const smoke = path.join(directory, "smoke");
  await mkdir(smoke);
  await writeFile(state, JSON.stringify(registry));
  await writeFile(promoted, "");
  await writeFile(path.join(directory, "manifest.json"), JSON.stringify(manifest));
  await writeFile(path.join(directory, "npm-cli.mjs"), npmStub);
  for (const [workspace, receipt] of Object.entries(receipts)) {
    await writeFile(path.join(smoke, `${workspace}.json`), JSON.stringify(receipt));
  }
  const result = spawnSync(
    process.execPath,
    [fileURLToPath(new URL("./publish.mjs", import.meta.url)), "promote"],
    {
      encoding: "utf8",
      env: {
        ...process.env,
        EXPECTED_COMMIT: manifest.source,
        NODE_AUTH_TOKEN: "token",
        PROMOTED: promoted,
        REGISTRY: state,
        RELEASE_MANIFEST: path.join(directory, "manifest.json"),
        SMOKE_RECEIPTS: smoke,
        npm_execpath: path.join(directory, "npm-cli.mjs"),
      },
    },
  );
  return { ...result, promoted: (await readFile(promoted, "utf8")).split("\n").filter(Boolean) };
};

const receiptFor = (workspace) => {
  const item = manifest.packages.find((candidate) => candidate.workspace === workspace);
  return { schema: 1, workspace, name: item.name, version: item.version, integrity: item.integrity };
};

test("promotion moves each package on its own evidence, in dependency order", async () => {
  const result = await promote({
    model: receiptFor("model"),
    "model-openai": receiptFor("model-openai"),
  });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.promoted, ["@aexhq/model@0.1.2", "@aexhq/model-openai@0.1.3"]);
});

test("promotion fails closed when the exact version has no passing component smoke", async () => {
  const missing = await promote({ model: receiptFor("model") });
  assert.notEqual(missing.status, 0);
  assert.deepEqual(missing.promoted, ["@aexhq/model@0.1.2"]);
  assert.match(missing.stderr, /1 package\(s\) stayed on next/u);

  // A receipt for other bytes is not evidence for these: componentize-js never rebuilds a
  // component byte-for-byte, so only the registry integrity ties a smoke to the staged archive.
  const stale = await promote({
    model: receiptFor("model"),
    "model-openai": { ...receiptFor("model-openai"), integrity: "sha512-rebuilt" },
  });
  assert.notEqual(stale.status, 0);
  assert.deepEqual(stale.promoted, ["@aexhq/model@0.1.2"]);
  assert.match(stale.stdout, /held @aexhq\/model-openai@0\.1\.3/u);
});
