import { z } from "zod";
import { unknown } from "./environment-server.mjs";

const serviceFrame = z.strictObject({ type: z.literal("service"), id: z.number().int().positive().safe(),
  method: z.enum(["emit", "result", "returned", "finish", "model", "environments"]), input: z.unknown() });

/** The untrusted child sees only JSON services for this invocation, never callback credentials. */
export async function toolProcess(op, child, { fetch = globalThis.fetch, signal, maxOutputBytes = 20 * 1024 * 1024 } = {}) {
  let bytes = 0;
  let buffer = "";
  let receipt;
  let finished = false;
  const pending = new Set();
  let writeError;
  const grant = op.request.callback;
  const decoder = new TextDecoder();
  const service = async frame => {
    let answer;
    try {
      if (finished || !grant.methods.includes(frame.method)) throw new Error("execution service is not granted");
      const response = await fetch(grant.url, { method: "POST", signal,
        headers: { authorization: `Bearer ${grant.token}`, "content-type": "application/json" },
        body: JSON.stringify({ method: frame.method, input: frame.input }) });
      if (!response.ok) throw new Error(`execution service failed: ${response.status}`);
      answer = { id: frame.id, output: await response.json() };
      if (frame.method === "finish") finished = true;
    } catch (error) { answer = { id: frame.id, error: String(error.message) }; }
    await child.write(JSON.stringify(answer) + "\n");
  };
  await child.write(JSON.stringify({ implementation: op.request.implementation, input: op.request.input,
    sessionId: op.session_id, environment: op.environment, sequence: op.sequence,
    ...(op.request.deadline_ms === undefined ? {} : { deadline_at_ms: Date.now() + op.request.deadline_ms }) }) + "\n");
  for await (const chunk of child.stdout) {
    bytes += Buffer.byteLength(chunk);
    if (bytes > maxOutputBytes) throw new Error("Tool process exceeded its output limit");
    buffer += typeof chunk === "string" ? chunk : decoder.decode(chunk, { stream: true });
    let newline;
    while ((newline = buffer.indexOf("\n")) !== -1) {
      const frame = JSON.parse(buffer.slice(0, newline));
      buffer = buffer.slice(newline + 1);
      if (receipt) throw new Error("Tool process wrote after its receipt");
      if (frame.type === "service") {
        const task = service(serviceFrame.parse(frame));
        pending.add(task);
        void task.catch(error => { writeError ??= error; }).finally(() => pending.delete(task));
      } else if (frame.type === "returned" || frame.type === "failure") receipt = frame;
      else throw new Error("unknown Tool process frame");
    }
  }
  await Promise.all(pending);
  if (writeError) throw writeError;
  if (buffer.trim()) throw new Error("Tool process ended inside a frame");
  if (receipt?.type === "returned" && !finished) return unknown("Tool process returned without a committed finish");
  return receipt ?? unknown("Tool process ended without a receipt");
}
