// The smallest Model an author can write with `@aexhq/model`: every export reports through the
// package's `typed` guard and every chunk reaches the package's SSE decoder as the component ABI
// delivers it. Compiled against the staged package, this is the only place those two contracts run
// inside a real component.
import { httpRead, httpStart } from "aex:model/host@1.0.0";
import { SseDecoder, parseJson, terminal, typed } from "@aexhq/model";

const attempts = new Map();

export function start(request) {
  return typed("start", () => {
    const options = parseJson(request.providerOptionsJson, "providerOptionsJson");
    const started = httpStart(request.operationId, {
      method: "POST",
      url: `${options.baseUrl}/v1/smoke`,
      headers: [["content-type", "application/json"]],
      body: new TextEncoder().encode(request.generationJson),
      credential: undefined,
      deadlineAtMs: request.deadlineAtMs,
    });
    attempts.set(started.requestId, { decoder: new SseDecoder(), sequence: 0 });
    return { providerOperationId: started.requestId };
  });
}

export function observe(providerOperationId, cursor) {
  return typed("observe", () => {
    const attempt = required(providerOperationId);
    const chunk = httpRead(providerOperationId, cursor, 64 * 1024);
    if (typeof chunk.status === "number" && (chunk.status < 200 || chunk.status >= 300)) {
      throw new Error(`smoke HTTP status ${chunk.status}`);
    }
    const events = [];
    for (const frame of attempt.decoder.feed(chunk.bytes)) {
      attempt.sequence += 1;
      events.push({
        cursor: `${chunk.cursor}:${attempt.sequence}`,
        kind: "text-delta",
        payloadJson: JSON.stringify({ index: 0, text: parseJson(frame.data, "smoke frame").text }),
      });
    }
    return {
      state: chunk.done ? "completed" : "streaming",
      events,
      nextCursor: chunk.cursor,
      terminalJson: chunk.done ? terminal("end_turn") : undefined,
    };
  });
}

export function cancel(providerOperationId) {
  return typed("cancel", () => {
    required(providerOperationId);
    attempts.delete(providerOperationId);
  });
}

export function acknowledge(providerOperationId) {
  return typed("acknowledge", () => {
    required(providerOperationId);
    attempts.delete(providerOperationId);
  });
}

function required(providerOperationId) {
  const attempt = attempts.get(providerOperationId);
  if (attempt === undefined) throw new Error(`unknown Model attempt ${providerOperationId}`);
  return attempt;
}
