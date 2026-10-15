import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import { randomUUID, createHash, createHmac } from "node:crypto";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const digest = bytes => createHash("sha256").update(bytes).digest("hex");
async function until(read, accept, label) {
  let value;
  for (const end = Date.now() + 20000; Date.now() < end; await delay(40)) {
    value = await read();
    if (accept(value)) return value;
  }
  assert.fail(`${label}: ${JSON.stringify(value)}`);
}

test("J01/J03/J09/J10/J12/J15/J23/J24: packed portable loop mediates real service work, approval, planned restart and parallel children", { timeout: 120000 }, async t => {
  assert(process.env.npm_execpath, "run through npm run test:portable");
  assert(process.env.BRAIN_TEST_V2_BINARY, "BRAIN_TEST_V2_BINARY must identify the admitted v2 service binary");
  assert(process.env.BRAIN_V2_PACKAGE_ARCHIVE, "BRAIN_V2_PACKAGE_ARCHIVE must identify its immutable SDK archive");
  const binary = path.resolve(process.env.BRAIN_TEST_V2_BINARY);
  const sdkArchive = path.resolve(process.env.BRAIN_V2_PACKAGE_ARCHIVE);
  await rm(path.join(root, ".build", "portable-service-evidence.json"), { force: true });
  const directory = path.join(root, ".test-output", "portable-" + randomUUID());
  const consumer = path.join(directory, "consumer"), artifacts = path.join(directory, "artifacts");
  const processes = [];
  let model, application, evidence;
  const stop = async child => {
    if (child && child.exitCode === null && child.signalCode === null) {
      const exited = once(child, "exit");
      child.kill();
      await exited;
    }
  };
  t.after(async () => {
    for (const child of processes) await stop(child);
    for (const service of [model, application]) if (service) {
      service.closeAllConnections();
      await new Promise(resolve => service.close(resolve));
    }
    await rm(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    if (evidence) {
      await mkdir(path.join(root, ".build"), { recursive: true });
      await writeFile(path.join(root, ".build", "portable-service-evidence.json"), JSON.stringify(evidence, null, 2) + "\n");
    }
  });
  await mkdir(consumer, { recursive: true });
  await mkdir(artifacts);
  const npm = (args, cwd) => execFileSync(process.execPath, [process.env.npm_execpath, ...args], {
    cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, TMP: directory, TEMP: directory, TMPDIR: directory }, timeout: 60000,
  });
  const [packed] = JSON.parse(npm(["pack", "--json", "--pack-destination", artifacts, "-w", "packages/loop-portable"], root));
  const archive = path.join(artifacts, packed.filename);
  assert.equal("sha512-" + createHash("sha512").update(await readFile(archive)).digest("base64"), packed.integrity);
  await writeFile(path.join(consumer, "package.json"), JSON.stringify({ name: "portable-service-consumer", private: true, type: "module" }));
  npm(["install", "--ignore-scripts", "--no-audit", "--no-fund", "--save-exact", sdkArchive, archive], consumer);
  await writeFile(path.join(consumer, "client.mjs"), 'export { SessionClient } from "@aexhq/brain/v2";\nexport { portable } from "@aexhq/agentloop-portable";\n');
  await writeFile(path.join(consumer, "types.ts"), `import { portable } from "@aexhq/agentloop-portable";
import type { SessionConfig } from "@aexhq/brain/v2";
const selected: SessionConfig["harness"] = portable({ delegation: true });
void selected;
// @ts-expect-error Startup executable selection is not a session option.
portable({ command: "unadmitted" });
// @ts-expect-error Delegation is explicitly boolean.
portable({ delegation: "yes" });
`);
  execFileSync(process.execPath, [path.join(root, "node_modules", "typescript", "bin", "tsc"), "--noEmit",
    "--strict", "--target", "ES2023", "--module", "NodeNext", "--moduleResolution", "NodeNext", "types.ts"],
  { cwd: consumer, stdio: "pipe", timeout: 30000 });
  const { SessionClient, portable } = await import(pathToFileURL(path.join(consumer, "client.mjs")));
  const counts = { model: 0, application: 0, parallelChildren: 0 };
  const requests = [], pendingChildren = [];
  model = createServer(async (request, response) => {
    let body = "";
    for await (const chunk of request) body += chunk;
    assert.equal(request.url, "/v1/chat/completions");
    const input = JSON.parse(body);
    requests.push(input);
    counts.model++;
    const latest = input.messages.at(-1);
    const user = input.messages.findLast(message => message.role === "user")?.content;
    let message = { role: "assistant", content: `answer: ${user}` };
    if (latest.role === "tool") {
      const value = JSON.parse(latest.content);
      message.content = value.kind === "untrusted_delegated_observations" ? "combined original children"
        : JSON.stringify({ delivered: value.delivered, citation: "artifact:review.json" });
    } else if (user === "review document") {
      message = { role: "assistant", content: null, tool_calls: [{ id: "deliver-review", type: "function",
        function: { name: "deliver", arguments: '{"document":"review"}' } }] };
    } else if (user === "delegate two") {
      message = { role: "assistant", content: null, tool_calls: [{ id: "delegate-review", type: "function",
        function: { name: "brain_delegate", arguments: '{"tasks":[{"message":"helper one"},{"message":"helper two"}]}' } }] };
    }
    const finish = () => {
      response.writeHead(200, { "Content-Type": "application/json" });
      response.end(JSON.stringify({ id: "fixture-" + counts.model, choices: [{ message }], usage: { prompt_tokens: 3 } }));
    };
    if (user?.startsWith("helper ") && latest.role !== "tool") {
      pendingChildren.push(finish);
      if (pendingChildren.length === 2) {
        counts.parallelChildren = pendingChildren.length;
        for (const complete of pendingChildren.splice(0)) complete();
      }
    } else finish();
  });
  application = createServer(async (request, response) => {
    let body = "";
    for await (const chunk of request) body += chunk;
    const execution = JSON.parse(body);
    assert.equal(request.headers["x-brain-signature"], "v1=" + createHmac("sha256", "fixture-signing").update(body).digest("hex"));
    assert.equal(request.headers["x-brain-operation"], execution.operation.id);
    assert.deepEqual(execution.arguments, { document: "review" });
    counts.application++;
    response.writeHead(200, { "Content-Type": "application/json" });
    response.end(JSON.stringify({ request_id: execution.operation.id, outcome: "completed", result: { delivered: counts.application } }));
  });
  for (const service of [model, application]) { service.listen(0, "127.0.0.1"); await once(service, "listening"); }
  let current, origin;
  const start = async () => {
    const child = spawn(binary, [], { cwd: consumer, stdio: ["ignore", "pipe", "pipe"], env: {
      PATH: process.env.PATH, SystemRoot: process.env.SystemRoot, USERPROFILE: process.env.USERPROFILE, HOME: process.env.HOME,
      BRAIN_DATA: path.join(directory, "data"), BRAIN_LISTEN: "127.0.0.1:0", BRAIN_API_TOKEN: "fixture-operator",
      BRAIN_NODE_NAME: "portable-artifact", BRAIN_DEVELOPMENT: "1", BRAIN_WORKERS: "4", BRAIN_LEASE_MS: "6000",
      BRAIN_L2_SHARDS: "2", BRAIN_L2_BYTES: "33554432", BRAIN_DIRTY_BYTES: "16777216", BRAIN_L1_BYTES: "1048576",
      BRAIN_HARNESSES: JSON.stringify([{ name: "portable", version: packed.version, command: process.execPath,
        arguments: [path.join(consumer, "node_modules", "@aexhq", "agentloop-portable", "bin", "brain-agentloop-portable.mjs")],
        models: ["fixture"], workers: 4, continuation: true }]),
      BRAIN_MODELS: JSON.stringify({ fixture: { kind: "openai", base_url: `http://127.0.0.1:${model.address().port}/v1/`, credential: "model" } }),
      BRAIN_SECRET_REFS: JSON.stringify({ local: { model: "FIXTURE_MODEL", signing: "FIXTURE_SIGNING" } }),
      FIXTURE_MODEL: "fixture-not-live", FIXTURE_SIGNING: "fixture-signing",
    } });
    processes.push(child);
    let log = "";
    child.stdout.on("data", chunk => { log += chunk; });
    child.stderr.on("data", chunk => { log += chunk; });
    await until(() => log, value => {
      assert.equal(child.exitCode, null, value);
      return /listen=127\.0\.0\.1:\d+/u.test(value);
    }, "packed portable service readiness");
    current = child;
    origin = "http://" + log.match(/listen=(127\.0\.0\.1:\d+)/u)[1];
    assert.equal((await fetch(origin + "/health/ready")).status, 204);
    return new SessionClient({ url: origin, token: "fixture-operator" });
  };
  let client = await start();
  const config = { model: { provider: "fixture", name: "fixture" }, harness: portable(), instructions: "Review the supplied document.",
    actions: [{ name: "deliver", description: "Deliver the review after approval", connection: "application",
      input_schema: { type: "object", required: ["document"], properties: { document: { type: "string" } }, additionalProperties: false },
      max_output_bytes: 8192, timeout_ms: 60000, approval: true }],
    connections: [{ name: "application", kind: "http", provider: "http", ownership: "borrowed",
      configuration: { url: `http://127.0.0.1:${application.address().port}` }, secrets: { signing: "signing" } }],
    events: [], grants: ["session:read", "input:write"],
    limits: { calls: 16, concurrent: 4, input_bytes: 131072, output_bytes: 65536, retained_bytes: 16777216, pending_inputs: 64 } };
  let session = await client.create(config);
  await session.attach("application", { requestId: "application" });
  const completed = (value, count) => until(() => value.runs(), runs => runs.length === count &&
    runs.every(run => run.status === "completed"), "original runs complete");
  await session.send("hello", "greeting");
  await completed(session, 1);
  assert.equal(counts.model, 1);
  const prefix = await session.records("0", 128);
  await session.send("review document", "review");
  const [approval] = await until(() => session.interactions(), values => values.length === 1, "durable exact approval");
  assert.equal(approval.status, "pending");
  assert.equal(counts.application, 0);
  const identifier = session.id;
  await until(async () => ({ records: await session.records("0", 128), runs: await session.runs() }),
    value => value.records.gaps.length === 0 &&
      value.records.records.some(record => record.kind === "interaction.requested" && record.data.id === approval.id) &&
      value.records.records.some(record => record.kind === "operation.prepared" && record.data.id === approval.request.subject) &&
      value.runs.length === 2 && value.runs.some(run => run.status === "waiting" && run.wait?.kind === "dispatch" && run.step === 1),
  "planned restart waits for the original durable approval boundary");
  await stop(current);
  client = await start();
  session = client.session(identifier);
  assert.equal((await session.interactions())[0].id, approval.id);
  await assert.rejects(session.answer(approval.id, { request_id: "wrong", revision: approval.revision,
    subject_digest: "wrong", accept: true, answer: true }), error => error.code === "conflict");
  await session.answer(approval.id, { request_id: "approved", revision: approval.revision,
    subject_digest: approval.request.subject_digest, accept: true, answer: true });
  await completed(session, 2);
  assert.deepEqual(counts, { model: 3, application: 1, parallelChildren: 0 });
  const reviewed = await session.records(prefix.next, 128);
  assert(reviewed.records.every(record => BigInt(record.sequence) > BigInt(prefix.next)));
  const answer = reviewed.records.find(record => record.kind === "run.completed").data.result;
  const structured = JSON.parse(answer.content);
  assert.deepEqual(structured, { delivered: 1, citation: "artifact:review.json" });
  const artifact = await session.putFile("review.json", new TextEncoder().encode(JSON.stringify(structured)), "application/json");
  assert.equal(artifact.name, "review.json");
  assert.deepEqual(JSON.parse(new TextDecoder().decode(await session.file("review.json"))), structured);
  await session.send("continue", "continue");
  await completed(session, 3);
  assert(requests.at(-1).messages.some(message => message.role === "assistant" && message.content === "answer: hello"));
  assert.equal((await session.read()).lifecycle, "open");
  const group = await client.create({ ...config, harness: portable({ delegation: true }), actions: [], connections: [] }, { message: "delegate two" });
  const [parent] = await completed(group, 1);
  const children = await group.children(parent.id, 1);
  assert.equal(children.ready, true);
  assert.deepEqual(children.children.map(child => child.status), ["completed", "completed"]);
  assert.equal(counts.parallelChildren, 2);
  assert.equal(counts.model, 8);
  assert.equal(counts.application, 1);
  for (const operation of await session.operations()) if (operation.kind === "model") {
    assert.equal(operation.usage.input_tokens, 3);
    assert.equal(Object.hasOwn(operation.usage, "output_tokens"), false);
  }
  evidence = {
    kind: "local-packed-artifact-public-service", published: false, portable: { version: packed.version, integrity: packed.integrity },
    sdkArchive: { sha256: digest(await readFile(sdkArchive)) }, binary: { sha256: digest(await readFile(binary)) },
    counts, retainedSession: identifier, features: ["typescript-public-imports", "approval", "journaled-restart", "artifact", "parallel-children", "sparse-usage"],
  };
});
