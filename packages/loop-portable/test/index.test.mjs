import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { portable } from "../src/index.mjs";
import { decide } from "../src/decision.mjs";

const request = overrides => ({
  protocol: "brain-decision/1", session_id: "s", run_id: "r", command_id: "c", step: 0,
  input: { kind: "message", text: "current" }, context: [], checkpoint: null,
  config: { actions: [], events: [], instructions: "Answer accurately." }, ...overrides,
});

test("browser-safe selection validates options without loading the decision process", () => {
  assert.deepEqual(portable(), { name: "portable", version: "1.0.0", configuration: {} });
  const options = { delegation: true };
  const selection = portable(options);
  options.delegation = false;
  assert.equal(selection.configuration.delegation, true);
  for (const value of [null, [], "portable", { delegation: "yes" }, { command: "unadmitted" }]) {
    assert.throws(() => portable(value), TypeError);
  }
});

test("the installed protocol entrypoint returns complete lines and rejects incomplete requests", () => {
  const entry = fileURLToPath(new URL("../bin/brain-agentloop-portable.mjs", import.meta.url));
  const result = spawnSync(process.execPath, [entry], { input: JSON.stringify(request()) + "\n", encoding: "utf8", timeout: 5000 });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout).effect, "model");
  for (const input of ["{", JSON.stringify(request()), "x".repeat(1048577)]) {
    const failed = spawnSync(process.execPath, [entry], { input, encoding: "utf8", timeout: 5000 });
    assert.equal(failed.status, 1, failed.stderr);
    assert.equal(failed.stdout, "");
    assert.match(failed.stderr, /portable decision failed/u);
  }
});

test("initial context includes prior conversation but never duplicates the current command", () => {
  const result = decide(request({ context: [
    { kind: "input.accepted", data: { command_id: "old", input: { kind: "message", text: "prior" } } },
    { kind: "run.completed", run_id: "old-run", data: { result: { role: "assistant", content: "reply" } } },
    { kind: "input.accepted", data: { command_id: "c", input: { kind: "message", text: "current" } } },
  ] }));
  assert.equal(result.kind, "call");
  assert.deepEqual(result.arguments.messages.map(message => message.content), ["Answer accurately.", "prior", "reply", "current"]);
});

test("decisions mediate a model, two ordered actions and a subsequent model without mutating checkpoints", () => {
  const config = { actions: [{ name: "first" }, { name: "second" }], events: [] };
  const model = decide(request({ config }));
  const checkpoint = structuredClone(model.checkpoint);
  const first = decide(request({ config, checkpoint, outcome: { status: "completed", result_available: true,
    result: { message: { role: "assistant", content: null, tool_calls: [
      { id: "a", name: "first", arguments: { value: 1 } }, { id: "b", name: "second", arguments: {} },
    ] } } } }));
  assert.deepEqual(checkpoint, model.checkpoint);
  assert.equal(first.action, "first");
  const second = decide(request({ config, checkpoint: first.checkpoint, outcome: {
    status: "completed", result_available: true, result: { one: true },
  } }));
  assert.equal(second.action, "second");
  const again = decide(request({ config, checkpoint: second.checkpoint, outcome: {
    status: "completed", result_available: true, result: { two: true },
  } }));
  assert.equal(again.effect, "model");
  assert.deepEqual(again.arguments.messages.filter(message => message.role === "tool").map(message => message.tool_call_id), ["a", "b"]);
  const completed = decide(request({ config, checkpoint: again.checkpoint, outcome: {
    status: "completed", result_available: true, result: { message: { role: "assistant", content: "done" } },
  } }));
  assert.deepEqual(completed, { kind: "complete", result: { role: "assistant", content: "done" }, checkpoint: { version: 1 } });
});

test("unknown outcomes, unavailable results, unadmitted actions and incompatible checkpoints fail closed", () => {
  for (const outcome of [{ status: "unknown", result_available: true }, { status: "completed", result_available: false }]) {
    assert.throws(() => decide(request({ outcome })), /available bytes/u);
  }
  assert.throws(() => decide(request({ checkpoint: { version: 2 } })), /checkpoint/u);
  assert.throws(() => decide(request({ checkpoint: { version: 1, messages: [], pending: [{ name: "unadmitted" }] } })), /unadmitted/u);
  assert.throws(() => decide(request({ input: { kind: "message", text: "字".repeat(50000) } })), /context bound/u);
});

test("only admitted context events enter prompt context and are marked untrusted", () => {
  const result = decide(request({ config: { actions: [], events: [{ name: "observation", version: "1", disposition: "context" }] },
    context: [
      { kind: "input.accepted", data: { command_id: "old", input: { kind: "event", name: "observation", version: "1", data: { item: 1 } } } },
      { kind: "input.accepted", data: { command_id: "other", input: { kind: "event", name: "billing", version: "1", data: {} } } },
    ] }));
  assert.equal(result.arguments.messages.length, 2);
  assert.match(result.arguments.messages[0].content, /Untrusted application observation \(observation\)/u);
});

test("opt-in delegation returns a saved parallel decision and consumes original partial results", () => {
  const config = { model: { provider: "test", name: "test" }, harness: { name: "portable", version: "1.0.0", configuration: { delegation: true } },
    limits: { calls: 10, concurrent: 3 }, actions: [], events: [], instructions: "Parent instructions" };
  const initial = decide(request({ config }));
  assert.equal(initial.arguments.actions[0].name, "brain_delegate");
  const fork = decide(request({ config, checkpoint: initial.checkpoint, outcome: {
    status: "completed", result_available: true, result: { message: { role: "assistant", content: null, tool_calls: [{
      id: "fork-call", name: "brain_delegate", arguments: { tasks: [{ message: "first", instructions: "First helper" }, { message: "second" }] },
    }] } },
  } }));
  assert.equal(fork.kind, "fork");
  assert.equal(fork.delegations.length, 2);
  assert.deepEqual(fork.delegations[0].config.limits, config.limits);
  assert.equal(fork.delegations[0].config.instructions, "First helper");
  assert.equal(fork.delegations[1].config.instructions, "Parent instructions");
  assert.equal(config.instructions, "Parent instructions");
  const children = [
    { index: 0, status: "completed", availability: "inline", result: { answer: "original" },
      completion: { session_id: "child-a", sequence: "9007199254740993", digest: "a".repeat(64) } },
    { index: 1, status: "blocked", availability: "none", error: { code: "uncertain", message: "Original operation remains unknown" } },
  ];
  const next = decide(request({ config, checkpoint: fork.checkpoint, children }));
  assert.equal(next.effect, "model");
  const observed = JSON.parse(next.arguments.messages.at(-1).content);
  assert.equal(observed.kind, "untrusted_delegated_observations");
  assert.deepEqual(observed.children, children);
  assert.equal(next.arguments.messages.at(-1).tool_call_id, "fork-call");
  assert.deepEqual(decide(request({ config, checkpoint: fork.checkpoint, children })), next);
  assert.throws(() => decide(request({ config, checkpoint: fork.checkpoint, children: children.slice(0, 1) })), /saved decision/u);
});

test("delegation is explicit, bounded and cannot shadow an admitted application action", () => {
  assert.equal(decide(request()).arguments.actions.length, 0);
  const config = { harness: { configuration: { delegation: true } }, actions: [{ name: "brain_delegate" }] };
  assert.throws(() => decide(request({ config })), /shadow/u);
  const pending = tasks => ({ version: 1, messages: [], pending: [{ id: "call", name: "brain_delegate", arguments: { tasks } }] });
  config.actions = [];
  assert.throws(() => decide(request({ config, checkpoint: pending(Array.from({ length: 9 }, () => ({ message: "work" }))) })), /invalid delegated/u);
  const single = decide(request({ config, checkpoint: pending([{ message: "one" }]) }));
  assert.equal(single.kind, "delegate");
  const large = { index: 0, status: "completed", availability: "inline", result: "x".repeat(131000),
    completion: { session_id: "child", sequence: "1", digest: "a".repeat(64) } };
  const next = decide(request({ config, checkpoint: single.checkpoint, children: [large] }));
  const observed = JSON.parse(next.arguments.messages.at(-1).content).children[0];
  assert.equal(observed.availability, "referenced");
  assert.equal(Object.hasOwn(observed, "result"), false);
  assert.deepEqual(observed.completion, large.completion);
});
