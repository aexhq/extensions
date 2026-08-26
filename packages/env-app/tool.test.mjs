import assert from "node:assert/strict";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import test from "node:test";

import { transpile } from "@bytecodealliance/jco";

import { dispatch } from "./dispatcher.mjs";

const here = new URL("./", import.meta.url);
const request = {
  metadata: {
    tenantId: "acc_1",
    sessionId: "ses_1",
    turnId: "trn_1",
    callId: "call_1",
    toolName: "verifyCallback",
  },
  inputJson: JSON.stringify({ value: "AEX_CALLBACK_OK" }),
  configJson: JSON.stringify({
    descriptor: { registration: "app.one", name: "verifyCallback", contract_digest: "a".repeat(64) },
  }),
  deadlineAtMs: 1n << 40n,
};
const terminal = JSON.stringify({
  value_json: JSON.stringify({ value: "AEX_CALLBACK_OK" }),
  content: "AEX_CALLBACK_OK",
  is_error: false,
});

test("the Tool hands the host exactly the arguments its contract declares", () => {
  const calls = [];
  const outcome = dispatch((...args) => {
    calls.push(args);
    return terminal;
  }, request);
  const wit = "aex:tool/environment@1.0.0";
  assert.deepEqual(calls, [[
    "call_1",
    JSON.stringify(JSON.parse(request.configJson).descriptor),
    request.inputJson,
    request.deadlineAtMs,
  ]], `${wit} takes the declared four arguments in order`);
  assert.deepEqual(outcome, {
    valueJson: JSON.stringify({ value: "AEX_CALLBACK_OK" }),
    content: "AEX_CALLBACK_OK",
    isError: false,
  });
});

// A surplus or missing argument is a wasm trap, never a type error, so the count is pinned against
// the contract the build compiles this component against rather than against a copy of it.
test("the declared host call and the Tool contract agree on arity", async () => {
  const wit = await readFile(new URL(import.meta.resolve("@aexhq/brain/contracts/tool")), "utf8");
  const declaration = /interface environment \{.*?invoke: func\(([^)]*)\)/su.exec(wit)?.[1];
  assert.ok(declaration !== undefined, "the Tool contract declares aex:tool/environment.invoke");
  const declared = declaration
    .split(",")
    .map((parameter) => parameter.split(":")[0].trim())
    .filter((parameter) => parameter !== "");
  assert.deepEqual(declared, ["operation-id", "descriptor-json", "input-json", "deadline-at-ms"]);

  let passed = 0;
  dispatch((...args) => {
    passed = args.length;
    return terminal;
  }, request);
  assert.equal(passed, declared.length);
});

// componentize-js rethrows an escaping `Error` as a bare `unreachable` trap that reaches Brain with
// no reason at all, which is how a stray argument reached a live plane as an unexplained failure.
// Run the built component so the guard is judged where the trap happens.
test("the built component reports a failure as its declared extension-error", async () => {
  const out = new URL("./dist/transpiled/", here);
  await rm(out, { recursive: true, force: true });
  await mkdir(out, { recursive: true });
  await writeFile(
    new URL("./stubs.mjs", out),
    `const state = globalThis.__aexToolStub;
export function invoke(...args) { state.calls.push(args); return state.terminal; }
export function metadata() { return state.metadata; }
export function cancelled() { return false; }
export function log() {}
export function read() { return { itemsJson: "[]" }; }
export function listObjects() { return { itemsJson: "[]" }; }
export function stat() { return "{}"; }
export function write() { return "{}"; }
function removeObject() {}
export { removeObject as delete };
export function spawn() { return "{}"; }
export function send() { return "{}"; }
export function inspect() { return "{}"; }
export function wait() { return "{}"; }
export function events() { return { itemsJson: "[]" }; }
export function manage() { return "{}"; }
export function listChildren() { return { itemsJson: "[]" }; }
`,
  );
  const component = await readFile(new URL("./dist/tool.component.wasm", here));
  const hosts = ["environment", "context", "journal", "storage", "children", "parent"];
  const { files } = await transpile(component, {
    name: "tool",
    map: Object.fromEntries(hosts.map((host) => [`aex:tool/${host}@1.0.0`, "./stubs.mjs"])),
  });
  for (const [name, bytes] of Object.entries(files)) {
    const separator = name.lastIndexOf("/");
    if (separator > 0) await mkdir(new URL(name.slice(0, separator), out), { recursive: true });
    await writeFile(new URL(name, out), bytes);
  }

  globalThis.__aexToolStub = { calls: [], terminal, metadata: request.metadata };
  const tool = await import(new URL("./tool.js", out).href);
  assert.deepEqual(tool.invoke(request), {
    valueJson: JSON.stringify({ value: "AEX_CALLBACK_OK" }),
    content: "AEX_CALLBACK_OK",
    isError: false,
  });
  assert.equal(globalThis.__aexToolStub.calls[0].length, 4);

  globalThis.__aexToolStub.terminal = "not a terminal";
  let failure;
  try {
    tool.invoke(request);
  } catch (error) {
    failure = error;
  }
  assert.ok(failure !== undefined, "an invalid terminal must fail the call");
  assert.equal(failure.payload?.code, "app_tool_invoke_failed");
  assert.match(failure.payload?.message ?? "", /JSON/u);
  assert.equal(failure.payload?.retryable, false);
});
