import assert from "node:assert/strict";
import test from "node:test";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { createLocalEnvironment } from "../dist/server.mjs";
import { commands, eventually } from "../../../shared/environment-test-fixture.mjs";

const exec = promisify(execFile);
const docker = async (...args) => (await exec("docker", args)).stdout.trim();
const image = process.env.BRAIN_WORKSPACE_IMAGE ?? "aex-workspace:test";
const invocation = (name, input, deadline_ms = 15_000) => ({ implementation: { type: "aex_official_tool", version: 1, name }, input, deadline_ms });

test("Docker workspace retains files, enforces profiles, cancels descendants and reports lost resources", { timeout: 120_000 }, async t => {
  const directory = await mkdtemp(join(tmpdir(), "aex-local-test-"));
  const profiles = { writer: { image, workspace: "write" }, reader: { image, workspace: "read" } };
  const env = await createLocalEnvironment({ directory, profiles });
  const command = commands();
  const reader = commands();
  t.after(async () => {
    for (const cmd of [command, reader]) {
      const receipt = (await env.handle(cmd("teardown"))).receipt;
      assert.equal(receipt.type, "accepted", JSON.stringify(receipt));
    }
    await rm(directory, { recursive: true });
  });
  assert.equal((await env.handle(command("setup", { configuration: { profile: "writer" } }))).receipt.type, "accepted");
  assert.equal((await env.handle(reader("setup", { configuration: { profile: "reader" } }))).receipt.type, "accepted");
  const writes = await Promise.all(["first", "second"].map(path => env.handle(command("execute", invocation("write", { path, content: path })))));
  assert.deepEqual(writes.map(value => value.receipt.type), ["result", "result"]);
  const saved = await Promise.all((await readdir(directory)).filter(name => name.endsWith(".json")).map(async name => JSON.parse(await readFile(join(directory, name), "utf8"))));
  const state = saved.find(value => value.profileName === "writer");
  assert.equal(state.phase, "ready");
  assert.equal((await env.handle(command("detach"))).receipt.type, "accepted");
  const restarted = await createLocalEnvironment({ directory, profiles });
  assert.equal((await restarted.handle(command("execute", invocation("read", { path: "first" })))).receipt.output.content, "first");
  assert.equal((await env.handle(reader("execute", invocation("write", { path: "forbidden", content: "x" })))).receipt.type, "failure");
  assert.equal((await env.handle(reader("execute", invocation("read", { path: "first" })))).receipt.type, "failure");
  const deniedNetwork = (await env.handle(command("execute", invocation("bash", { command: "node -e 'fetch(\"http://1.1.1.1\").then(()=>process.exit(9)).catch(()=>console.log(\"denied\"))'" })))).receipt;
  assert.equal(deniedNetwork.output.stdout.trim(), "denied");
  assert.equal((await env.handle(command("execute", { implementation: { type: "arbitrary" }, input: {}, deadline_ms: 1000 }))).receipt.code, "unsupported");
  const pending = command("execute", invocation("bash", { command: "echo entered > started; (sleep 60; echo late > late) & wait" }, 90_000));
  const running = env.handle(pending);
  let container;
  await eventually(async () => {
    container = await docker("ps", "--filter", `volume=${state.volume}`, "--format", "{{.ID}}");
    if (!container) return false;
    try { await docker("exec", container, "test", "-f", "/workspace/started"); return true; }
    catch { return false; }
  });
  assert.equal((await env.handle(command("cancel", { target_sequence: pending.operation.sequence }))).receipt.type, "accepted");
  const cancelled = (await running).receipt;
  assert.equal(cancelled.type, "failure");
  assert.equal(cancelled.code, "cancelled");
  await assert.rejects(docker("inspect", container));
  assert.equal((await env.handle(command("execute", invocation("read", { path: "late" })))).receipt.type, "failure");
  const timedOut = (await env.handle(command("execute", invocation("bash", { command: "sleep 60" }, 2_000)))).receipt;
  assert.equal(timedOut.type, "failure");
  assert.equal(timedOut.code, "timeout");
  assert.equal(await docker("ps", "--all", "--filter", `label=aex.binding=${state.volume}`, "--format", "{{.ID}}"), "");
  await docker("volume", "rm", state.volume);
  const lost = (await restarted.handle(command("execute", invocation("read", { path: "first" })))).receipt;
  assert.equal(lost.code, "resource_lost");
  await assert.rejects(docker("volume", "inspect", state.volume));
});

test("a preprepared locked Python project runs offline before any import-time dependency is needed", { timeout: 30_000 }, async t => {
  const directory = await mkdtemp(join(tmpdir(), "aex-python-test-"));
  const env = await createLocalEnvironment({ directory, profiles: {
    python: { image: process.env.BRAIN_PYTHON_IMAGE ?? "aex-workspace-python:test", workspace: "read" },
  } });
  const command = commands();
  t.after(async () => {
    assert.equal((await env.handle(command("teardown"))).receipt.type, "accepted");
    await rm(directory, { recursive: true });
  });
  const setup = (await env.handle(command("setup", { configuration: { profile: "python" } }))).receipt;
  assert.equal(setup.type, "accepted", JSON.stringify(setup));
  const request = { implementation: { type: "python_project", name: "versions" }, input: { version: "1.2.3rc1" }, deadline_ms: 15_000 };
  const results = await Promise.all([env.handle(command("execute", request)), env.handle(command("execute", request))]);
  for (const { receipt } of results) assert.deepEqual(receipt, { type: "result", output: { normalized: "1.2.3rc1", prerelease: true } });
});

test("Docker cleanup failure retains execution evidence and teardown can reclaim the invocation", { timeout: 30_000 }, async t => {
  const directory = await mkdtemp(join(tmpdir(), "aex-cleanup-test-"));
  const wrapper = join(directory, "docker-fixture");
  await writeFile(wrapper, '#!/bin/sh\nif [ "$1" = rm ] && [ -f "$0.fail" ]; then echo "fixture Docker cleanup unavailable" >&2; exit 1; fi\nexec docker "$@"\n', { mode: 0o700 });
  await writeFile(`${wrapper}.fail`, "");
  const env = await createLocalEnvironment({ directory, docker: wrapper, profiles: { test: { image, workspace: "write" } } });
  const command = commands();
  t.after(async () => {
    await rm(`${wrapper}.fail`, { force: true });
    assert.equal((await env.handle(command("teardown"))).receipt.type, "accepted");
    await rm(directory, { recursive: true });
  });
  assert.equal((await env.handle(command("setup", { configuration: { profile: "test" } }))).receipt.type, "accepted");
  const receipt = (await env.handle(command("execute", invocation("write", { path: "committed", content: "yes" })))).receipt;
  assert.equal(receipt.type, "failure");
  assert.equal(receipt.code, "cleanup_failed");
  assert.ok(receipt.details.output);
  assert.match(receipt.message, /workspace cleanup failed/u);
  const stateFile = (await readdir(directory)).find(name => name.endsWith(".json"));
  const state = JSON.parse(await readFile(join(directory, stateFile), "utf8"));
  const retained = await docker("ps", "--all", "--filter", `label=aex.binding=${state.volume}`, "--format", "{{.ID}}");
  assert.ok(retained);
  const timeout = (await env.handle(command("execute", invocation("bash", { command: "sleep 3" }, 2_000)))).receipt;
  assert.equal(timeout.code, "timeout");
  assert.match(timeout.details.cleanup_error, /fixture Docker cleanup unavailable/u);
  await rm(`${wrapper}.fail`);
  assert.equal((await env.handle(command("teardown"))).receipt.type, "accepted");
  await assert.rejects(docker("inspect", retained));
  await assert.rejects(docker("volume", "inspect", state.volume));
});
