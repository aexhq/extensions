import { invoke as invokeEnvironment } from "aex:tool/environment@1.0.0";

export function invoke(request) {
  const config = JSON.parse(request.configJson);
  const terminal = invokeEnvironment(
    request.metadata.callId,
    JSON.stringify(config.descriptor),
    decodeBase64(config.bundleBase64),
    request.inputJson,
    request.deadlineAtMs,
  );
  const parsed = JSON.parse(terminal);
  if (
    parsed !== null &&
    typeof parsed === "object" &&
    typeof parsed.value_json === "string" &&
    typeof parsed.content === "string" &&
    typeof parsed.is_error === "boolean"
  ) {
    return {
      valueJson: parsed.value_json,
      content: parsed.content,
      isError: parsed.is_error,
    };
  }
  return { valueJson: terminal, content: terminal, isError: false };
}

function decodeBase64(value) {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}
