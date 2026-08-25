import { httpCancel, httpRead, httpStart } from "aex:model/host@1.0.0";
import { SseDecoder, terminal } from "@aexhq/model";
import { buildRequest, decodeFrame } from "./provider.mjs";

const attempts = new Map();

export function start(request) {
  const started = httpStart(request.operationId, buildRequest(request));
  attempts.set(started.requestId, { requestId: started.requestId, decoder: new SseDecoder(), cursor: undefined, sequence: 0, stopReason: undefined, completed: false });
  return { providerOperationId: started.requestId };
}

export function observe(providerOperationId, cursor) {
  const attempt = requiredAttempt(providerOperationId);
  if (attempt.completed) return { state: "completed", events: [], nextCursor: attempt.cursor, terminalJson: terminal(attempt.stopReason) };
  const chunk = httpRead(attempt.requestId, cursor ?? attempt.cursor, 64 * 1024);
  if (chunk.status !== undefined && (chunk.status < 200 || chunk.status >= 300)) throw new Error(`OpenAI HTTP status ${chunk.status}`);
  const events = [];
  for (const frame of attempt.decoder.feed(chunk.bytes)) {
    for (const decoded of decodeFrame(frame.event, frame.data)) {
      if (decoded.stopReason !== undefined) attempt.stopReason = decoded.stopReason;
      if (decoded.kind === undefined) continue;
      attempt.sequence += 1;
      events.push({ cursor: `${chunk.cursor}:${attempt.sequence}`, kind: decoded.kind, payloadJson: JSON.stringify(decoded.payload) });
    }
  }
  attempt.cursor = chunk.cursor;
  if (chunk.done) {
    if (attempt.decoder.pending !== 0) throw new Error("OpenAI stream ended inside an SSE frame");
    attempt.completed = true;
  }
  return { state: chunk.done ? "completed" : "streaming", events, nextCursor: chunk.cursor, terminalJson: chunk.done ? terminal(attempt.stopReason) : undefined };
}

export function cancel(providerOperationId) { const attempt = requiredAttempt(providerOperationId); httpCancel(attempt.requestId); attempt.completed = true; attempt.stopReason = "cancelled"; }
export function acknowledge(providerOperationId) { if (!attempts.delete(providerOperationId)) throw new Error(`unknown Model attempt ${providerOperationId}`); }
function requiredAttempt(id) { const attempt = attempts.get(id); if (attempt === undefined) throw new Error(`unknown Model attempt ${id}`); return attempt; }
