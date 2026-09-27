import { isDeepStrictEqual } from "node:util";
import { randomBytes } from "node:crypto";
import { z } from "zod";
import { descriptor, response, completion } from "./protocol.mjs";
import { publicJson } from "./outbound.mjs";
import { environmentHandler, bindingKey, accepted, failure, unknown, result, fail, deadlineTimer, finishExecution } from "../../../shared/environment-server.mjs";
export { serveEnvironment } from "../../../shared/environment-server.mjs";

export function createHttpEnvironment({ authorize, request = publicJson, fetch = globalThis.fetch }) {
  if (typeof authorize !== "function") throw new TypeError("authorize is required");
  const running = new Map();
  const callbacks = new Map();
  const callback = async (headers, body) => {
    const token = headers.authorization?.replace(/^Bearer /u, "");
    const execution = callbacks.get(token);
    if (!execution || execution.controller.signal.aborted || execution.finishing) fail("denied", "invocation is not available");
    const call = z.strictObject({ method: z.enum(["result", "returned", "finish"]), input: z.json() }).parse(body);
    if (!execution.grant.methods.includes(call.method)) fail("denied", "invocation method is not granted");
    if (call.method === "finish") execution.finishing = true;
    const reply = await fetch(execution.grant.url, { method: "POST", redirect: "error", signal: execution.controller.signal,
      headers: { authorization: `Bearer ${execution.grant.token}`, "content-type": "application/json" }, body: JSON.stringify(call) });
    if (!reply.ok) throw new Error("invocation update was not acknowledged");
    const sequence = z.number().int().positive().safe().parse(await reply.json());
    if (call.method === "finish") execution.committed = true;
    return sequence;
  };
  const handle = environmentHandler(async (op, context) => {
    const key = bindingKey(op);
    const input = { sessionId: op.session_id, environment: op.environment };
    if (op.request.type === "call" && op.request.name === "inspect") {
      z.strictObject({}).parse(op.request.input);
      const binding = await authorize(input, undefined, context);
      return result({ tools: binding.tools.map(tool => tool.name), timeoutMs: binding.timeoutMs });
    }
    if (op.request.type === "setup") {
      const configuration = z.strictObject({ binding: z.string().min(1).optional(), authorization: z.string().min(1).optional() })
        .refine(value => value.binding !== undefined || value.authorization !== undefined).parse(op.request.configuration);
      await authorize({ ...input, configuration }, undefined, context);
      return accepted();
    }
    if (op.request.type === "cancel") {
      running.get(`${key}/${op.request.target_sequence}`)?.abort("cancelled");
      return accepted();
    }
    if (["detach", "teardown"].includes(op.request.type)) {
      for (const [id, controller] of running) if (id.startsWith(`${key}/`)) controller.abort("cancelled");
      return accepted();
    }
    if (op.request.type !== "execute") fail("unsupported", "HTTP Environment only executes tools");
    const implementation = descriptor.parse(op.request.implementation);
    const started = Date.now();
    const controller = new AbortController();
    const identity = `${key}/${op.sequence}`;
    running.set(identity, controller);
    let cancelTimer = deadlineTimer(op.request.deadline_ms, () => controller.abort("timeout"));
    let dispatched = false;
    let relayToken;
    let execution;
    try {
      const binding = await authorize(input, controller.signal, context);
      if (!binding.tools.some(tool => isDeepStrictEqual(tool, implementation.definition))) fail("denied", "Tool is outside the approved binding");
      const timeout = z.number().int().positive().max(300_000).parse(binding.timeoutMs);
      cancelTimer();
      const remaining = Math.min(timeout, (op.request.deadline_ms ?? Infinity) - (Date.now() - started));
      cancelTimer = deadlineTimer(remaining, () => controller.abort("timeout"));
      controller.signal.throwIfAborted();
      const body = { contract: "http-tool/v1", ...input, sequence: op.sequence, tool: implementation.definition,
        input: op.request.input, deadlineAtMs: Date.now() + remaining };
      if (implementation.type === "application_tool") {
        const url = z.url().parse(binding.callbackUrl);
        relayToken = randomBytes(32).toString("hex");
        execution = { grant: op.request.callback, controller, finishing: false, committed: false };
        callbacks.set(relayToken, execution);
        Object.assign(body, { contract: "http-tool/v2", options: implementation.options,
          callback: { url, token: relayToken, methods: op.request.callback.methods.filter(method => ["result", "returned", "finish"].includes(method)) } });
      }
      dispatched = true;
      const raw = await request(binding.url, { token: binding.token, signal: controller.signal, body });
      if (execution?.committed) return accepted();
      controller.signal.throwIfAborted();
      if (execution !== undefined) {
        const rejected = response.safeParse(raw);
        if (rejected.success && rejected.data.type === "failure") return failure(rejected.data.code, rejected.data.message);
        if (!completion.safeParse(raw).success) return unknown("application response was invalid; invocation was not retried");
        return unknown("application completion was not acknowledged; invocation was not retried");
      }
      const reply = response.parse(raw);
      const receipt = reply.type === "success" ? result(reply.value) : failure(reply.code, reply.message);
      return await finishExecution(op, receipt, (url, options) => fetch(url, { ...options, signal: controller.signal }));
    } catch (error) {
      if (execution?.committed) return accepted();
      if (controller.signal.aborted) return failure(controller.signal.reason, "HTTP invocation interrupted; external effects may have completed");
      if (dispatched) return unknown("application response was lost or invalid; invocation was not retried");
      throw error;
    } finally { cancelTimer(); running.delete(identity); if (relayToken !== undefined) callbacks.delete(relayToken); }
  });
  return { handle, callback };
}
