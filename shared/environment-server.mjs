import { BrainError } from "@aexhq/brain";
import { environmentProtocol, finishExecution as finish, reportEnvironment as report } from "@aexhq/brain/runtime";
import { z } from "zod";
export { serveEnvironment } from "@aexhq/brain/runtime";

export const identifier = z.string().regex(/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u);
export class EnvironmentError extends BrainError {
  constructor(code, message) { super(400, code, message, false); }
}
export const fail = (code, message) => { throw new EnvironmentError(code, message); };
export const accepted = () => ({ type: "accepted" });
export const result = output => ({ type: "result", output });
export const failure = (code, message, details) => ({ type: "failure", code,
  message: String(message).slice(0, 4096), retryable: false, ...(details === undefined ? {} : { details }) });
export const unknown = message => ({ type: "unknown", message: String(message).slice(0, 4096) });
export const bindingKey = op => `${op.session_id}/${op.environment}${(op.binding?.sequence ?? 1) === 1 ? "" : `/${op.binding.sequence}`}`;

export async function reportEnvironment(reporter, observation, fetch = globalThis.fetch) {
  if (reporter !== undefined) return report(reporter, { type: "result", output: { observation } }, { fetch });
}
export const finishExecution = (operation, receipt, fetch = globalThis.fetch) => finish(operation, receipt, { fetch });

export function deadlineTimer(milliseconds, expire) {
  let timer;
  const deadline = milliseconds === undefined ? undefined : Date.now() + milliseconds;
  const schedule = () => {
    if (deadline === undefined) return;
    const remaining = deadline - Date.now();
    if (remaining <= 0) expire();
    else timer = setTimeout(schedule, Math.min(remaining, 2_147_483_647));
  };
  schedule();
  return () => clearTimeout(timer);
}

export function environmentHandler(handle) {
  return environmentProtocol(async (operation, context) => {
    if (operation.request.type === "execute" && !operation.request.callback?.methods.includes("finish")) {
      fail("not_granted", "Tool completion service must be granted");
    }
    try { return await handle(operation, context); }
    catch (error) {
      if (error instanceof BrainError) throw error;
      fail("environment_failed", error.message ?? error);
    }
  });
}
