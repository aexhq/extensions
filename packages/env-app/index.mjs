import { createHash, timingSafeEqual } from "node:crypto";
import { createServer } from "node:http";

const CONTRACT = "environment/v1";
const MAX_BODY_BYTES = 2 * 1024 * 1024;

export function createEnvironment(options = {}) {
  const tools = new Map(Object.entries(options.tools ?? {}));
  const environments = new Map();
  const receipts = options.receipts ?? new Map();
  const maxReceipts = options.maxReceipts ?? 10_000;
  const active = new Map();

  return async function handle(command) {
    validateCommand(command);
    const operation = command.operation;
    const prior = receipts.get(operation.operation_id);
    if (prior !== undefined) {
      if (prior.request_digest !== operation.request_digest) {
        return response(operation, { type: "conflict", expected_digest: prior.request_digest, actual_digest: operation.request_digest });
      }
      return prior.response;
    }
    if (receipts.size >= maxReceipts) {
      return response(operation, { type: "failure", code: "receipt_capacity", message: "Environment receipt capacity is exhausted", retryable: true });
    }
    const controller = new AbortController();
    active.set(operation.operation_id, controller);
    let receipt;
    try {
      receipt = await execute(operation, environments, tools, options, controller.signal, active);
    } catch (error) {
      receipt = { type: "failure", code: "environment_error", message: String(error?.message ?? error).slice(0, 4096), retryable: false };
    } finally {
      active.delete(operation.operation_id);
    }
    const terminal = response(operation, receipt);
    receipts.set(operation.operation_id, { request_digest: operation.request_digest, response: terminal });
    return terminal;
  };
}

export function serveEnvironment(options = {}) {
  const handle = createEnvironment(options);
  const token = options.token;
  const server = createServer(async (request, response) => {
    if (request.method !== "POST" || request.url !== "/v1/operations") return send(response, 404, { code: "not_found" });
    if (token !== undefined && !authorized(request.headers.authorization, token)) return send(response, 401, { code: "unauthorized" });
    try {
      const body = await readBody(request);
      send(response, 200, await handle(JSON.parse(body)));
    } catch (error) {
      send(response, 400, { code: "invalid_request", message: String(error?.message ?? error) });
    }
  });
  return {
    server,
    listen(port = 8090, host = "127.0.0.1") {
      return new Promise((resolve, reject) => {
        server.once("error", reject);
        server.listen(port, host, () => { server.off("error", reject); resolve(server.address()); });
      });
    },
    close() { return new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve())); },
  };
}

async function execute(operation, environments, tools, options, signal, active) {
  const request = operation.request;
  switch (request.type) {
    case "setup": {
      const digest = digestOf(request.configuration);
      const existing = environments.get(operation.environment_id);
      if (existing !== undefined && existing.configurationDigest !== digest) {
        return { type: "conflict", expected_digest: existing.configurationDigest, actual_digest: digest };
      }
      if (existing === undefined) {
        await options.setup?.({ environmentId: operation.environment_id, configuration: request.configuration, signal });
        environments.set(operation.environment_id, { configurationDigest: digest, configuration: request.configuration, attachments: new Map() });
      }
      return { type: "accepted" };
    }
    case "attach": {
      const environment = requiredEnvironment(environments, operation.environment_id);
      if (operation.attachment_id === undefined) throw new Error("attach requires attachment_id");
      environment.attachments.set(operation.attachment_id, { sessionId: operation.session_id, grants: request.grants });
      await options.attach?.({ environmentId: operation.environment_id, attachmentId: operation.attachment_id, sessionId: operation.session_id, grants: request.grants, signal });
      return { type: "accepted" };
    }
    case "execute": {
      const environment = requiredEnvironment(environments, operation.environment_id);
      const attachment = environment.attachments.get(operation.attachment_id);
      if (attachment === undefined || attachment.sessionId !== operation.session_id) throw new Error("attachment does not authorize this session");
      const tool = tools.get(request.remote_tool_id);
      if (typeof tool !== "function") throw new Error(`unknown remote Tool ${request.remote_tool_id}`);
      const output = await tool(request.tool.input, { signal, grant: request.grant, configuration: environment.configuration, workspace: environment.configuration?.workspace, deadlineMs: Date.now() + 120_000, environmentId: operation.environment_id, sessionId: operation.session_id, attachmentId: operation.attachment_id });
      return { type: "tool_result", result: { call_id: request.tool.call_id, output, is_error: false } };
    }
    case "call":
      return { type: "result", output: await options.call?.({ name: request.name, input: request.input, operation, signal }) };
    case "cancel":
      active.get(request.target_operation_id)?.abort();
      await options.cancel?.({ targetOperationId: request.target_operation_id, operation });
      return { type: "accepted" };
    case "detach": {
      const environment = requiredEnvironment(environments, operation.environment_id);
      if (operation.attachment_id === undefined) throw new Error("detach requires attachment_id");
      environment.attachments.delete(operation.attachment_id);
      await options.detach?.({ environmentId: operation.environment_id, attachmentId: operation.attachment_id, sessionId: operation.session_id, signal });
      return { type: "accepted" };
    }
    case "teardown": {
      const environment = requiredEnvironment(environments, operation.environment_id);
      if (environment.attachments.size !== 0) throw new Error("cannot tear down an Environment with active attachments");
      await options.teardown?.({ environmentId: operation.environment_id, configuration: environment.configuration, signal });
      environments.delete(operation.environment_id);
      return { type: "accepted" };
    }
    default:
      throw new Error("unsupported Environment request");
  }
}

function validateCommand(command) {
  if (command?.contract !== CONTRACT || command.binding === null || typeof command.binding !== "object" || command.operation === null || typeof command.operation !== "object") throw new Error("invalid environment/v1 command");
  const operation = command.operation;
  for (const name of ["operation_id", "request_digest", "environment_id", "session_id"]) {
    if (typeof operation[name] !== "string" || operation[name].length === 0) throw new Error(`operation ${name} is required`);
  }
  if (!/^[0-9a-f]{64}$/u.test(operation.request_digest) || operation.request === null || typeof operation.request !== "object") throw new Error("operation digest or request is invalid");
  if (command.binding.environment_id !== operation.environment_id || !/^[0-9a-f]{64}$/u.test(command.binding.configuration_digest) || typeof command.binding.adapter_binding !== "string" || command.binding.adapter_binding.length === 0 || !Number.isSafeInteger(command.binding.directory_generation) || command.binding.directory_generation < 1 || !["session", "shared", "external"].includes(command.binding.lifecycle_policy)) throw new Error("Environment binding is invalid");
}

function response(operation, receipt) {
  return { contract: CONTRACT, operation_id: operation.operation_id, request_digest: operation.request_digest, receipt };
}

function requiredEnvironment(environments, id) {
  const environment = environments.get(id);
  if (environment === undefined) throw new Error(`Environment ${id} is not set up`);
  return environment;
}

function digestOf(value) {
  return createHash("sha256").update(canonical(value)).digest("hex");
}

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value !== null && typeof value === "object") return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  return JSON.stringify(value);
}

function authorized(header, expected) {
  if (typeof header !== "string" || !header.startsWith("Bearer ")) return false;
  const actual = Buffer.from(header.slice(7));
  const wanted = Buffer.from(expected);
  return actual.length === wanted.length && timingSafeEqual(actual, wanted);
}

async function readBody(request) {
  const chunks = [];
  let bytes = 0;
  for await (const chunk of request) {
    bytes += chunk.length;
    if (bytes > MAX_BODY_BYTES) throw new Error("request body exceeds 2 MiB");
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
}

function send(response, status, value) {
  const body = JSON.stringify(value);
  response.writeHead(status, { "content-type": "application/json", "content-length": Buffer.byteLength(body) });
  response.end(body);
}
