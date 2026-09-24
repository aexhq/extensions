import { isDeepStrictEqual } from "node:util";
import { inspectTool } from "@aexhq/brain";
import { z } from "zod";
import { invocation, response } from "./protocol.mjs";

export function createToolHandler({ tools, authorize, maxBodyBytes = 1_048_576 }) {
  if (typeof authorize !== "function") throw new TypeError("authorize is required");
  const registry = new Map();
  for (const placed of tools) {
    const tool = inspectTool(placed);
    if (!tool.handler || !tool.contract || registry.has(tool.definition.name)) throw new TypeError("HTTP tools need unique native handlers");
    registry.set(tool.definition.name, tool);
  }
  return async request => {
    if (request.method !== "POST") return new Response(null, { status: 405 });
    let call;
    try {
      let size = 0;
      const chunks = [];
      for await (const chunk of request.body ?? []) {
        size += chunk.byteLength;
        if (size > maxBodyBytes) return new Response(null, { status: 413 });
        chunks.push(chunk);
      }
      call = invocation.parse(JSON.parse(Buffer.concat(chunks).toString("utf8")));
    } catch { return new Response(null, { status: 400 }); }
    const reply = (code, message) => Response.json({ type: "failure", code, message });
    const remaining = Math.min(300_000, call.deadlineAtMs - Date.now());
    if (remaining <= 0) return reply("timeout", "invocation deadline expired");
    const signal = AbortSignal.any([request.signal, AbortSignal.timeout(remaining)]);
    const stopped = new Promise((_, reject) => signal.addEventListener("abort", () => reject(signal.reason), { once: true }));
    const bounded = action => Promise.race([Promise.resolve().then(() => { signal.throwIfAborted(); return action(); }), stopped]);
    try { await bounded(() => authorize(request, call)); }
    catch { return reply("denied", "invocation is not authorized"); }
    const tool = registry.get(call.tool.name);
    if (!tool || !isDeepStrictEqual(tool.definition, call.tool)) return reply("incompatible_tool", "registered contract differs from the session contract");
    let input;
    try { input = await bounded(() => tool.contract.input.parseAsync(call.input)); }
    catch { return reply("invalid_input", "input failed validation or expired"); }
    const unsupported = () => { throw new Error("HTTP handlers must return JSON; invocation services are not available"); };
    let output;
    try {
      signal.throwIfAborted();
      output = await bounded(() => tool.handler(input, { sessionId: call.sessionId, sequence: call.sequence,
        deadline: new Date(call.deadlineAtMs), signal,
        emit: unsupported, emitResult: unsupported, finish: unsupported, model: unsupported }));
    } catch {
      return reply(signal.aborted ? "timeout" : "tool_error", signal.aborted ? "invocation interrupted; effects may have completed" : "application handler failed");
    }
    try {
      if (tool.contract.output) output = await bounded(() => tool.contract.output.parseAsync(output));
      return Response.json(response.parse({ type: "success", value: z.json().parse(output) }));
    } catch { return reply("invalid_output", "output failed validation or expired"); }
  };
}
