import { isDeepStrictEqual } from "node:util";
import { z } from "zod";
import { descriptor, response } from "./protocol.mjs";
import { publicJson } from "./outbound.mjs";
import { environmentHandler, bindingKey, accepted, failure, unknown, result, fail, deadlineTimer, finishExecution } from "../../../shared/environment-server.mjs";
export { serveEnvironment } from "../../../shared/environment-server.mjs";

export function createHttpEnvironment({ authorize, request = publicJson, fetch = globalThis.fetch }) {
  if (typeof authorize !== "function") throw new TypeError("authorize is required");
  const running = new Map();
  const handle = environmentHandler(async op => {
    const key = bindingKey(op);
    const input = { sessionId: op.session_id, environment: op.environment };
    if (op.request.type === "setup") {
      const configuration = z.strictObject({ binding: z.string().min(1), authorization: z.string().min(1).optional() }).parse(op.request.configuration);
      await authorize({ ...input, configuration });
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
    try {
      const binding = await authorize(input, controller.signal);
      if (!binding.tools.some(tool => isDeepStrictEqual(tool, implementation.definition))) fail("denied", "Tool is outside the approved binding");
      const timeout = z.number().int().positive().max(300_000).parse(binding.timeoutMs);
      cancelTimer();
      const remaining = Math.min(timeout, (op.request.deadline_ms ?? Infinity) - (Date.now() - started));
      cancelTimer = deadlineTimer(remaining, () => controller.abort("timeout"));
      controller.signal.throwIfAborted();
      dispatched = true;
      const reply = response.parse(await request(binding.url, { token: binding.token, signal: controller.signal,
        body: { contract: "http-tool/v1", ...input, sequence: op.sequence, tool: implementation.definition,
          input: op.request.input, deadlineAtMs: Date.now() + remaining } }));
      controller.signal.throwIfAborted();
      const receipt = reply.type === "success" ? result(reply.value) : failure(reply.code, reply.message);
      return await finishExecution(op, receipt, (url, options) => fetch(url, { ...options, signal: controller.signal }));
    } catch (error) {
      if (controller.signal.aborted) return failure(controller.signal.reason, "HTTP invocation interrupted; external effects may have completed");
      if (dispatched) return unknown("application response was lost or invalid; invocation was not retried");
      throw error;
    } finally { cancelTimer(); running.delete(identity); }
  });
  return { handle };
}
