import { randomUUID } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";

export function commands(session = randomUUID(), environment = "test") {
  let sequence = 0;
  return (type, fields = {}) => ({ contract: "environment/v1", operation: {
    session_id: session, environment, sequence: ++sequence, request: { type, ...fields },
  } });
}
export async function eventually(check, timeoutMs = 15_000) {
  const until = Date.now() + timeoutMs;
  for (;;) {
    if (await check()) return;
    if (Date.now() >= until) throw new Error("condition did not become true before the test deadline");
    await delay(30);
  }
}
