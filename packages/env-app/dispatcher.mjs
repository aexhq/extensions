/**
 * The callback Tool's whole body. It takes the host import rather than importing it, so the
 * declared call can be exercised without a wasm runtime.
 */
export function dispatch(invokeEnvironment, request) {
  const config = JSON.parse(request.configJson);
  const terminal = invokeEnvironment(
    request.metadata.callId,
    JSON.stringify(config.descriptor),
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
  throw new Error("application Environment returned an invalid Tool terminal");
}
