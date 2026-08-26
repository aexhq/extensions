import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

test("exact dependency verification ignores a newer latest tag", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "extensions-dependencies-"));
  const manifest = path.join(directory, "manifest.json");
  const npm = path.join(directory, "npm-cli.mjs");
  const calls = path.join(directory, "calls.json");
  await writeFile(manifest, JSON.stringify({
    schema: 1,
    packages: [{
      name: "@aexhq/example-extension",
      version: "1.0.0",
      dependencies: { "@aexhq/brain": "0.3.1" },
    }],
  }));
  await writeFile(npm, `
    import { writeFileSync } from "node:fs";
    const args = process.argv.slice(2);
    writeFileSync(process.env.CALLS, JSON.stringify(args));
    if (args[1] === "@aexhq/brain@latest") process.stdout.write(JSON.stringify("0.3.2"));
    else if (args[1] === "@aexhq/brain@0.3.1") process.stdout.write(JSON.stringify({
      version: "0.3.1",
      "dist.integrity": "sha512-pinned",
    }));
    else process.exitCode = 1;
  `);

  const result = spawnSync(
    process.execPath,
    [fileURLToPath(new URL("./verify-dependencies.mjs", import.meta.url)), manifest],
    {
      encoding: "utf8",
      env: { ...process.env, npm_execpath: npm, CALLS: calls },
    },
  );

  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(await readFile(calls, "utf8")), [
    "view",
    "@aexhq/brain@0.3.1",
    "version",
    "dist.integrity",
    "--json",
  ]);
});

test("promotion verifies the staged manifest with the current workflow source", async () => {
  const workflow = await readFile(
    new URL("../.github/workflows/npm-publish.yml", import.meta.url),
    "utf8",
  );
  const promoteStart = workflow.search(/^  promote:\r?$/mu);
  assert.notEqual(promoteStart, -1);
  const promote = workflow.slice(promoteStart);
  assert.match(
    promote,
    /name: extensions-npm-release-\$\{\{ needs\.validate\.outputs\.release_sha \}\}/u,
  );
  assert.match(
    promote,
    /node tools\/verify-dependencies\.mjs "\$RUNNER_TEMP\/extensions-npm-release\/manifest\.json"/u,
  );
  assert.match(
    promote,
    /EXPECTED_COMMIT: \$\{\{ needs\.validate\.outputs\.release_sha \}\}/u,
  );
  // Without these the job cannot see the stage run's receipts, and every promotion fails closed
  // mid-release instead of at the pull request that dropped them.
  assert.match(
    promote,
    /pattern: extensions-npm-smoke-\$\{\{ needs\.validate\.outputs\.release_sha \}\}-\*/u,
  );
  assert.match(promote, /SMOKE_RECEIPTS: \$\{\{ runner\.temp \}\}\/extensions-npm-smoke/u);
  assert.doesNotMatch(promote, /node "\$RUNNER_TEMP\/extensions-npm-release\/verify-dependencies\.mjs"/u);
  assert.doesNotMatch(workflow, /test "\$release_sha" = "\$EXPECTED_COMMIT"/u);
});
