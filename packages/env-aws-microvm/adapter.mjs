import { dispatch } from "aex:environment/host@1.0.0";

const NO_DEADLINE = 18_446_744_073_709_551_615n;

export function resolve(request) {
  const config = JSON.parse(request.configJson);
  return {
    bindingJson: JSON.stringify({
      driver: config.driver,
      configuration: config.configuration,
      authority: JSON.parse(request.authorityJson),
      tenant_id: request.tenantId,
      session_id: request.sessionId,
      root_id: request.rootId,
      parent_id: request.parentId,
      environment_id: request.environmentId,
    }),
  };
}

export function submit(bindingJson, operation) {
  const response = call(operation.operationId, "submit", {
    binding: JSON.parse(bindingJson),
    operation: {
      operation_id: operation.operationId,
      kind: operation.kind,
      descriptor_json: operation.descriptorJson,
      bundle_base64: operation.bundle === undefined ? undefined : encodeBase64(operation.bundle),
      input_json: operation.inputJson,
      deadline_at_ms: operation.deadlineAtMs.toString(),
    },
  }, operation.deadlineAtMs);
  return { providerOperationId: response.provider_operation_id ?? operation.operationId };
}

export function observe(bindingJson, providerOperationId, cursor) {
  const response = call(providerOperationId, "observe", {
    binding: JSON.parse(bindingJson),
    provider_operation_id: providerOperationId,
    cursor,
  });
  return {
    state: response.state,
    cursor: response.cursor ?? cursor ?? "",
    chunksJson: JSON.stringify(response.chunks ?? []),
    terminalJson:
      response.terminal_json === undefined
        ? undefined
        : typeof response.terminal_json === "string"
          ? response.terminal_json
          : JSON.stringify(response.terminal_json),
  };
}

export function cancel(bindingJson, providerOperationId) {
  call(providerOperationId, "cancel", {
    binding: JSON.parse(bindingJson),
    provider_operation_id: providerOperationId,
  });
}

export function acknowledge(bindingJson, providerOperationId, terminalJson) {
  call(providerOperationId, "acknowledge", {
    binding: JSON.parse(bindingJson),
    provider_operation_id: providerOperationId,
    terminal: JSON.parse(terminalJson),
  });
}

export function release(bindingJson) {
  const binding = JSON.parse(bindingJson);
  call(binding.environment_id, "release", { binding });
}

function call(operationId, action, request, deadlineAtMs = NO_DEADLINE) {
  return JSON.parse(dispatch(operationId, action, JSON.stringify(request), deadlineAtMs));
}

function encodeBase64(bytes) {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}
