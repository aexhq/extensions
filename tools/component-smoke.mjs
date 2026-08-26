/**
 * Run one package's published components against the real host ABI.
 *
 *   node tools/component-smoke.mjs <workspace> [--from <package directory>] [--receipt <file>]
 *
 * `--from` selects the package under test: the working tree by default, or a clean install of the
 * exact staged version. The host is Brain's own `component-host` worker (`COMPONENT_HOST`), so
 * these rounds cross the same `contracts/*` boundary the kernel crosses — the only place an
 * opaque Wasm trap, a null sampling field, a dropped sealed instruction or a `list<u8>` the guest
 * does not recognise becomes visible before a tagged deployment.
 */
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createInterface } from "node:readline";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

const root = path.resolve(import.meta.dirname, "..");
const fixtures = path.join(import.meta.dirname, "component-smoke");
const componentHost = process.env.COMPONENT_HOST;
if (componentHost === undefined) {
  throw new Error("COMPONENT_HOST must name Brain's component-host binary");
}

class Host {
  #child;
  #pending = new Map();
  #next = 1;
  #capabilities = () => {
    throw new Error("the scenario bound no capability handler");
  };

  constructor(binary) {
    this.#child = spawn(binary, [], { stdio: ["pipe", "pipe", "inherit"] });
    this.#child.on("exit", (code, signal) => {
      const reason = new Error(`the component host exited with ${signal ?? code}`);
      for (const pending of this.#pending.values()) pending.reject(reason);
      this.#pending.clear();
    });
    createInterface({ input: this.#child.stdout }).on("line", (line) => {
      this.#frame(line).catch((error) => {
        this.#child.kill();
        throw error;
      });
    });
  }

  bind(capabilities) {
    this.#capabilities = capabilities;
  }

  close() {
    this.#child.stdin.end();
  }

  request(request) {
    const id = this.#next;
    this.#next += 1;
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      this.#write({ frame: "request", id, request });
    });
  }

  #write(frame) {
    this.#child.stdin.write(`${JSON.stringify(frame)}\n`);
  }

  async #frame(line) {
    const frame = JSON.parse(line);
    if (frame.frame === "response") {
      const pending = this.#pending.get(frame.id);
      this.#pending.delete(frame.id);
      if ("Ok" in frame.result) pending.resolve(frame.result.Ok);
      else pending.reject(new Error(frame.result.Err));
      return;
    }
    try {
      const value = await this.#capabilities(frame.call);
      this.#write({ frame: "capability_result", id: frame.id, result: { Ok: value ?? null } });
    } catch (error) {
      this.#write({
        frame: "capability_result",
        id: frame.id,
        result: {
          Err: {
            code: error.code ?? "smoke_capability_failed",
            message: String(error.message ?? error),
            retryable: false,
          },
        },
      });
    }
  }
}

const denied = (code, message) => Object.assign(new Error(message), { code });
const utf8 = (bytes) => Buffer.from(bytes).toString("utf8");
const sse = (frames) => frames.map((frame) => `${frame}\n\n`).join("");
const DEADLINE = 4_102_444_800_000;

/** Brain always sends every sampling field; an unset one arrives as an explicit JSON null. */
const sealedPrefix = (extra) => ({
  system_prompt: "Answer with one word.",
  max_tokens: null,
  temperature: null,
  stop_sequences: null,
  tool_choice_none: null,
  ...extra,
});

const sealedRequest = (body, absent) => {
  const value = JSON.parse(body);
  for (const field of absent) assert.equal(field in value, false, `${field} reached the provider`);
  assert.match(body, /Answer with one word\./u, "the sealed instructions never reached the provider");
};

async function model(host, target, component) {
  const started = [];
  const stream = target.chunks.map((chunk, index) => ({ cursor: `c${index}`, ...chunk }));
  host.bind((call) => {
    if (call.capability === "model.http.start") {
      started.push(call.request);
      return { request_id: "req-smoke" };
    }
    if (call.capability === "model.http.read") {
      const chunk = stream.shift();
      if (chunk === undefined) throw denied("smoke_stream", "the Model read past the scripted stream");
      return {
        cursor: chunk.cursor,
        // Only the first chunk carries a status, and a host `list<u8>` reaches a componentized
        // guest as a plain array of byte values, never a `Uint8Array`.
        status: chunk.status ?? null,
        headers: chunk.status === undefined ? [] : [["content-type", "text/event-stream"]],
        bytes: [...Buffer.from(chunk.body ?? "", "utf8")],
        done: chunk.done === true,
      };
    }
    throw denied("smoke_capability", `unscripted Model capability ${call.capability}`);
  });

  const request = {
    operation_id: "model_op_smoke",
    model: target.model,
    messages_json: JSON.stringify([{ role: "user", content: [{ type: "text", text: "hi" }] }]),
    tools_json: "[]",
    response_format_json: null,
    generation_json: JSON.stringify(target.generation),
    provider_options_json: JSON.stringify(target.options),
    deadline_at_ms: DEADLINE,
  };
  const attempt = await host.request({
    kind: "model_start",
    instance_id: "model-smoke",
    component,
    request,
  });
  assert.equal(typeof attempt.provider_operation_id, "string");
  assert.equal(started.length, 1);
  target.body(utf8(started[0].body));

  const texts = [];
  let observation;
  let cursor = null;
  do {
    observation = await host.request({
      kind: "model_observe",
      instance_id: "model-smoke",
      provider_operation_id: attempt.provider_operation_id,
      cursor,
    });
    cursor = observation.next_cursor;
    for (const event of observation.events) {
      if (event.kind === "TextDelta") texts.push(JSON.parse(event.payload_json).text);
    }
  } while (observation.state === "Streaming");
  assert.equal(observation.state, "Completed");
  assert.equal(texts.join(""), target.text);
  assert.equal(typeof observation.terminal_json, "string");
  await host.request({
    kind: "model_acknowledge",
    instance_id: "model-smoke",
    provider_operation_id: attempt.provider_operation_id,
    terminal_json: observation.terminal_json,
  });

  stream.push({ cursor: "c-failed", status: 500, body: "" });
  const failing = await host.request({
    kind: "model_start",
    instance_id: "model-smoke-failed",
    component,
    request,
  });
  await assert.rejects(
    host.request({
      kind: "model_observe",
      instance_id: "model-smoke-failed",
      provider_operation_id: failing.provider_operation_id,
      cursor: null,
    }),
    // componentize-js compiles a thrown plain `Error` into a Wasm trap whose message is gone; a
    // typed `extension-error` is the only way this reason reaches the kernel.
    /status 500/u,
    "the provider status never reached the kernel",
  );
  await host.request({ kind: "release", world: "model", instance_id: "model-smoke-failed" });
}

async function environment(host, _target, component) {
  const bundle = [...Buffer.from("smoke bundle", "utf8")];
  const observations = [
    { state: "running", cursor: "c1", chunks: [{ seq: 1, text: "working" }] },
    { state: "completed", cursor: "c2", chunks: [], terminal_json: { ok: true, exit_code: 0 } },
  ];
  let failing = false;
  host.bind((call) => {
    assert.equal(call.capability, "environment.dispatch");
    if (failing) throw denied("driver_unavailable", "the smoke driver refused the operation");
    const { action, request } = call.request;
    if (action === "submit") {
      assert.equal(utf8(Buffer.from(request.operation.bundle_base64, "base64")), "smoke bundle");
      return { provider_operation_id: "prov-smoke" };
    }
    if (action === "observe") return observations.shift();
    return {};
  });

  const resolve = {
    tenant_id: "ten_smoke",
    session_id: "ses_smoke",
    root_id: "ses_smoke",
    parent_id: null,
    environment_id: "workspace",
    config_json: JSON.stringify({ driver: { kind: "smoke" }, configuration: { region: "local" } }),
    policy_json: JSON.stringify({ network: "deny" }),
  };
  const operation = {
    operation_id: "env_op_smoke",
    kind: "invoke",
    descriptor_json: JSON.stringify({ runtime: "node22", tool_name: "smoke" }),
    bundle,
    input_json: JSON.stringify({ command: "true" }),
    deadline_at_ms: DEADLINE,
  };
  const resolved = await host.request({
    kind: "environment_resolve",
    instance_id: "env-smoke",
    component,
    request: resolve,
  });
  const binding = JSON.parse(resolved.binding_json);
  assert.equal(binding.session_id, "ses_smoke");
  assert.deepEqual(binding.policy, { network: "deny" });

  const submitted = await host.request({
    kind: "environment_submit",
    instance_id: "env-smoke",
    binding_json: resolved.binding_json,
    operation,
  });
  assert.equal(submitted.provider_operation_id, "prov-smoke");

  const observe = (cursor) => host.request({
    kind: "environment_observe",
    instance_id: "env-smoke",
    binding_json: resolved.binding_json,
    provider_operation_id: submitted.provider_operation_id,
    cursor,
  });
  const running = await observe(null);
  assert.equal(running.state, "Running");
  assert.deepEqual(JSON.parse(running.chunks_json), [{ seq: 1, text: "working" }]);
  const completed = await observe(running.cursor);
  assert.equal(completed.state, "Completed");
  assert.deepEqual(JSON.parse(completed.terminal_json), { ok: true, exit_code: 0 });

  await host.request({
    kind: "environment_acknowledge",
    instance_id: "env-smoke",
    binding_json: resolved.binding_json,
    provider_operation_id: submitted.provider_operation_id,
    terminal_json: completed.terminal_json,
  });
  await host.request({
    kind: "environment_release",
    instance_id: "env-smoke",
    binding_json: resolved.binding_json,
  });

  failing = true;
  await host.request({
    kind: "environment_resolve",
    instance_id: "env-smoke-failed",
    component,
    request: resolve,
  });
  await assert.rejects(
    host.request({
      kind: "environment_submit",
      instance_id: "env-smoke-failed",
      binding_json: resolved.binding_json,
      operation,
    }),
    /the smoke driver refused the operation/u,
    "the driver failure never reached the kernel",
  );
  await host.request({ kind: "release", world: "environment", instance_id: "env-smoke-failed" });
}

async function tool(host, target, component) {
  const config = target.configFile === undefined
    ? target.config ?? {}
    : JSON.parse(await readFile(path.join(target.from, target.configFile), "utf8"));
  let failing = false;
  host.bind((call) => {
    if (failing) throw denied("environment_unavailable", "the smoke grant refused the call");
    if (call.capability === "tool.environment.invoke") {
      assert.deepEqual(JSON.parse(call.request.descriptor_json), config.descriptor);
      assert.equal(call.request.bundle_base64 === null, config.bundleBase64 === undefined);
      return JSON.stringify({ value_json: JSON.stringify({ ok: true }), content: "ok", is_error: false });
    }
    if (call.capability === "tool.children.spawn") {
      assert.equal(JSON.parse(call.request.request_json).name, "smoke child");
      return JSON.stringify({ child_id: "chi_smoke", status: "running" });
    }
    throw denied("smoke_capability", `unscripted Tool capability ${call.capability}`);
  });

  const invocation = {
    metadata: {
      tenant_id: "ten_smoke",
      session_id: "ses_smoke",
      turn_id: "turn_smoke",
      call_id: "call_smoke",
      tool_name: target.toolName,
    },
    input_json: JSON.stringify(target.input),
    config_json: JSON.stringify(config),
    deadline_at_ms: DEADLINE,
  };
  const outcome = await host.request({
    kind: "tool",
    component,
    request: invocation,
    grants: [target.grant],
  });
  assert.equal(outcome.is_error, false);
  assert.deepEqual(JSON.parse(outcome.value_json), target.value);

  failing = true;
  await assert.rejects(
    host.request({ kind: "tool", component, request: invocation, grants: [target.grant] }),
    /the smoke grant refused the call/u,
    "the grant failure never reached the kernel",
  );
}

const SESSION = {
  session_id: "ses_smoke",
  model: "smoke-model",
  limits: { max_rounds_per_turn: 8, turn_wall_ms: 60_000, max_parallel_tools: 4 },
  metadata: { tools: [] },
};

const round = (answers, seen) => (call) => {
  assert.equal(call.capability, "agentloop.call");
  seen.push(call.request.op);
  const answer = answers[call.request.op.op];
  if (answer === undefined) throw denied("internal", `unscripted Agentloop op ${call.request.op.op}`);
  return JSON.stringify(answer(call.request.op));
};

const completedRound = {
  model_stream: () => ({
    result: {
      op: "model_stream",
      message: {
        content: [{ type: "text", text: "hello" }],
        stop_reason: "end_turn",
        model: SESSION.model,
        usage: { input_tokens: 3, output_tokens: 1 },
      },
    },
  }),
  tools_dispatch: ({ calls }) => ({
    result: {
      op: "tools_dispatch",
      results: calls.map((item) => ({
        tool_call_id: item.tool_call_id,
        name: item.name,
        is_error: false,
        content: [{ type: "text", text: "ok" }],
      })),
    },
  }),
  journal_append: () => ({ result: { op: "journal_append", first_seq: 5, last_seq: 5 } }),
  journal_read: () => ({ result: { op: "journal_read", entries: [] } }),
  kv_get: () => ({ result: { op: "kv_get", entries: {} } }),
  kv_set: () => ({ result: { op: "kv_set" } }),
  turn_finish: () => ({ result: { op: "turn_finish" } }),
  turn_fail: () => ({ result: { op: "turn_fail" } }),
};

async function agentloop(host, _target, component) {
  const activate = (kind, payload) => host.request({
    kind: "agentloop",
    instance_id: "loop-smoke",
    component,
    request: {
      operation_id: `act_${kind}`,
      session_id: SESSION.session_id,
      kind,
      payload_json: JSON.stringify(payload),
      config_json: JSON.stringify({ instructions: "Answer with one word." }),
      deadline_at_ms: DEADLINE,
    },
  });

  let seen = [];
  host.bind(round(completedRound, seen));
  const started = await activate("session_start", {
    activation_id: "act-start",
    session: SESSION,
    resumed: false,
    kv: {},
    tail: [
      {
        type: "user_message",
        seq: 1,
        at: "2026-08-25T00:00:00Z",
        content: [{ type: "text", text: "earlier" }],
      },
    ],
  });
  assert.equal(JSON.parse(started.payload_json).outcome, "completed");

  const message = {
    activation_id: "act-1",
    kind: "message",
    session: SESSION,
    message: { seq: 4, at: "2026-08-25T00:00:01Z", content: [{ type: "text", text: "say hello" }] },
  };
  const completed = await activate("message", message);
  assert.equal(JSON.parse(completed.payload_json).outcome, "completed");
  assert.ok(seen.some((op) => op.op === "model_stream"), "the loop composed no model round");
  assert.match(JSON.stringify(seen), /say hello/u, "the admitted message never reached the loop");
  assert.match(JSON.stringify(seen), /earlier/u, "the session_start tail never reached the loop");

  seen = [];
  host.bind(round({
    ...completedRound,
    model_stream: () => ({
      error: { code: "provider_error", message: "the smoke provider refused the round", retryable: false },
    }),
  }, seen));
  // A failed round has to reach the kernel as data carrying the provider's reason — through the
  // activation payload or a terminal op — never as a trap whose message is gone.
  const failed = await activate("message", { ...message, activation_id: "act-2" });
  assert.match(
    JSON.stringify([failed.payload_json, seen]),
    /the smoke provider refused the round/u,
    "the provider reason never reached the kernel",
  );
  await host.request({ kind: "release", world: "agentloop", instance_id: "loop-smoke" });
}

const scenarios = { model, environment, tool, agentloop };

const openai = {
  kind: "model",
  component: "dist/model.component.wasm",
  model: "gpt-smoke",
  options: { baseUrl: "https://provider.invalid", apiKey: "sk-smoke", outputTokenParameter: "max_completion_tokens" },
  generation: sealedPrefix({ reasoning_effort: null }),
  body: (body) => sealedRequest(body, ["temperature", "max_completion_tokens", "reasoning_effort", "stop", "tool_choice"]),
  chunks: [
    { status: 200, body: sse([`data: ${JSON.stringify({ choices: [{ delta: { content: "hel" } }] })}`]) },
    {
      body: sse([
        `data: ${JSON.stringify({ choices: [{ delta: { content: "lo" }, finish_reason: "stop" }], usage: { prompt_tokens: 3, completion_tokens: 1 } })}`,
        "data: [DONE]",
      ]),
      done: true,
    },
  ],
  text: "hello",
};

const anthropic = {
  ...openai,
  model: "claude-smoke",
  options: { baseUrl: "https://provider.invalid", apiKey: "sk-smoke" },
  // `reasoning_effort` is omitted: this dialect rejects the field outright, so Brain's real
  // prefix cannot be sent here until that guard also ignores an unset null.
  generation: sealedPrefix({ max_tokens: 128 }),
  body: (body) => sealedRequest(body, ["temperature", "stop_sequences"]),
  chunks: [
    {
      status: 200,
      body: sse([
        `event: content_block_delta\ndata: ${JSON.stringify({ index: 0, delta: { type: "text_delta", text: "hel" } })}`,
      ]),
    },
    {
      body: sse([
        `event: content_block_delta\ndata: ${JSON.stringify({ index: 0, delta: { type: "text_delta", text: "lo" } })}`,
        `event: message_delta\ndata: ${JSON.stringify({ delta: { stop_reason: "end_turn" }, usage: { output_tokens: 1 } })}`,
      ]),
      done: true,
    },
  ],
};

const environmentTarget = { kind: "environment", component: "dist/environment.component.wasm" };

const plan = {
  agentloop: [{ kind: "agentloop", component: buildAgentloopFixture }],
  "env-app": [
    environmentTarget,
    {
      kind: "tool",
      component: "dist/tool.component.wasm",
      config: { descriptor: { runtime: "callback", tool_name: "callback" } },
      grant: "environment",
      toolName: "callback",
      input: { command: "true" },
      value: { ok: true },
    },
  ],
  "env-aws-microvm": [environmentTarget],
  "loop-codex": [{ kind: "agentloop", component: "dist/loop.component.wasm" }],
  "loop-pi": [{ kind: "agentloop", component: "dist/loop.component.wasm" }],
  model: [
    {
      kind: "model",
      component: buildModelFixture,
      model: "smoke-model",
      options: { baseUrl: "https://provider.invalid" },
      generation: sealedPrefix({}),
      body: (body) => assert.match(body, /Answer with one word\./u),
      chunks: [
        { status: 200, body: sse([`data: ${JSON.stringify({ text: "hel" })}`]) },
        { body: sse([`data: ${JSON.stringify({ text: "lo" })}`]), done: true },
      ],
      text: "hello",
    },
  ],
  "model-anthropic": [anthropic],
  "model-openai": [openai],
  tools: [
    {
      kind: "tool",
      component: "dist/tool.component.wasm",
      configFile: "dist/bash.component.json",
      grant: "environment",
      toolName: "bash",
      input: { command: "true" },
      value: { ok: true },
    },
    {
      kind: "tool",
      component: "dist/children.component.wasm",
      grant: "children",
      toolName: "subagents",
      input: { action: "spawn_agent", message: "run the smoke", task_name: "smoke child" },
      value: { child_id: "chi_smoke", status: "running" },
    },
  ],
};

async function buildModelFixture(from, into) {
  const { build } = await import("esbuild");
  const bundled = await build({
    entryPoints: [path.join(fixtures, "model.fixture.mjs")],
    bundle: true,
    format: "esm",
    platform: "neutral",
    external: ["aex:model/host@1.0.0"],
    alias: { "@aexhq/model": path.join(from, "index.mjs") },
    write: false,
    legalComments: "none",
  });
  const { componentize } = await import("@bytecodealliance/componentize-js");
  const wit = await readFile(new URL(import.meta.resolve("@aexhq/brain/contracts/model")), "utf8");
  const output = await componentize(bundled.outputFiles[0].text, wit, {
    worldName: "model",
    disableFeatures: ["http", "fetch-event"],
  });
  const file = path.join(into, "model.fixture.component.wasm");
  await writeFile(file, output.component);
  return file;
}

async function buildAgentloopFixture(from, into) {
  const { buildAgentloopComponent } = await import(
    pathToFileURL(path.join(from, "dist", "build.js")).href
  );
  const { componentize } = await import("@bytecodealliance/componentize-js");
  const built = await buildAgentloopComponent(
    { entry: path.join(fixtures, "agentloop.fixture.mjs") },
    componentize,
  );
  const file = path.join(into, "agentloop.fixture.component.wasm");
  await writeFile(file, built.component);
  return file;
}

const [workspace, ...flags] = process.argv.slice(2);
const option = (name) => {
  const index = flags.indexOf(`--${name}`);
  return index < 0 ? undefined : flags[index + 1];
};
const targets = plan[workspace];
if (targets === undefined) throw new Error(`no component smoke is declared for ${workspace}`);
const from = path.resolve(option("from") ?? path.join(root, "packages", workspace));
const receipt = option("receipt");

const scratch = await mkdtemp(path.join(tmpdir(), "extensions-component-smoke-"));
const components = [];
try {
  for (const target of targets) {
    const file = typeof target.component === "string"
      ? path.join(from, target.component)
      : await target.component(from, scratch);
    const bytes = await readFile(file);
    const sha256 = createHash("sha256").update(bytes).digest("hex");
    const host = new Host(componentHost);
    try {
      await scenarios[target.kind](host, { ...target, from }, { path: file, sha256 });
    } finally {
      host.close();
    }
    components.push({ component: path.basename(file), kind: target.kind, sha256 });
    process.stdout.write(`${workspace}: ${target.kind} ${path.basename(file)} ${sha256}\n`);
  }
} finally {
  await rm(scratch, { recursive: true, force: true });
}

if (receipt !== undefined) {
  const document = JSON.parse(await readFile(path.join(from, "package.json"), "utf8"));
  await writeFile(receipt, `${JSON.stringify({
    schema: 1,
    workspace,
    name: document.name,
    version: document.version,
    integrity: process.env.SMOKE_INTEGRITY ?? null,
    components,
  }, null, 2)}\n`);
}
