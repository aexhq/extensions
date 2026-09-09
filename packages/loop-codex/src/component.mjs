import * as host from "brain:agentloop/host@0.1.0";

import { runCodex } from "./logic.mjs";

function hosted(call) {
  try {
    return call();
  } catch (error) {
    const payload = error !== null && typeof error === "object" && "payload" in error ? error.payload : undefined;
    const failure = new Error(payload?.message ?? String(error?.message ?? error));
    failure.code = payload?.code ?? "host_error";
    failure.retryable = payload?.retryable ?? false;
    throw failure;
  }
}

export async function turn(input) {
  try {
    const output = await runCodex({
      input: JSON.parse(input.inputJson),
      transcript: JSON.parse(input.transcriptJson),
      events: JSON.parse(input.eventsJson),
      configuration: JSON.parse(input.configurationJson),
      system: input.system,
      tools: JSON.parse(input.toolsJson),
    }, {
      setTranscript: (messages) => hosted(() => host.setTranscript(JSON.stringify(messages))),
      kv: {
        read: (key) => {
          const value = hosted(() => host.kvRead(key));
          return value === undefined ? undefined : JSON.parse(value);
        },
        put: (key, value) => hosted(() => host.kvPut(key, JSON.stringify(value))),
        delete: (key) => hosted(() => host.kvDelete(key)),
      },
      events: (after) => JSON.parse(hosted(() => host.events(BigInt(after)))),
      model: (request) => JSON.parse(hosted(() => host.model(JSON.stringify(request)))),
      dispatch: (calls) => JSON.parse(hosted(() => host.dispatch(JSON.stringify(calls)))),
      emit: (kind, data) => hosted(() => host.emit(kind, JSON.stringify(data ?? null))),
      telemetry: (record) => host.telemetry(JSON.stringify(record ?? null)),
    });
    return {
      resultJson: output.result === undefined ? undefined : JSON.stringify(output.result),
    };
  } catch (error) {
    const failure = new Error(String(error?.message ?? error) || "Agentloop turn failed");
    failure.payload = {
      code: typeof error?.code === "string" && error.code.length > 0 ? error.code : "agentloop_failed",
      message: failure.message,
      retryable: Boolean(error?.retryable),
    };
    throw failure;
  }
}
