import { createHash } from "node:crypto";
import { readFileSync, writeSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

const [bundlePath] = process.argv.slice(2);
if (bundlePath === undefined) {
  process.stderr.write("environment tool runner requires a bundle path\n");
  process.exit(70);
}

const abort = new AbortController();
process.on("SIGTERM", () => abort.abort(new Error("tool call cancelled")));
process.on("SIGINT", () => abort.abort(new Error("tool call cancelled")));

// The request is parsed and validated before any customer code can run: a malformed frame or a
// bad output ceiling must fail fast with a diagnostic, never after tool side effects.
let request;
try {
  request = JSON.parse(readFileSync(0, "utf8"));
  if (
    request === null
    || typeof request !== "object"
    || request.seal === null
    || typeof request.seal !== "object"
    || !Number.isSafeInteger(request.max_output_bytes)
    || request.max_output_bytes < 1
  ) {
    throw new TypeError("tool request frame is malformed");
  }
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`invalid tool request: ${message}\n`);
  process.exit(65);
}

let resultFrameStarted = false;

const writeResult = (maxOutputBytes, value, inline) => {
  const encodedInline = JSON.stringify(inline);
  if (encodedInline === undefined || Buffer.byteLength(encodedInline) > maxOutputBytes) {
    throw new RangeError("Tool result exceeds the sealed output ceiling");
  }
  const encoded = JSON.stringify(value);
  // Rust applies the authoritative RFC 8785 check to `inline`. This wrapper has only a fixed
  // discriminator and is allowed bounded headroom in the supervisor-owned IPC file.
  if (Buffer.byteLength(encoded) > maxOutputBytes + 4096) {
    throw new RangeError("Tool result envelope exceeds its bounded IPC ceiling");
  }
  const body = Buffer.from(encoded);
  const header = Buffer.allocUnsafe(4);
  header.writeUInt32BE(body.length);
  const frame = Buffer.concat([header, body]);
  // fd 3 is a fresh anonymous channel for this execution. Mark the frame started before its first
  // byte: if a short write then fails, emitting a second frame would create an ambiguous result.
  resultFrameStarted = true;
  let offset = 0;
  while (offset < frame.length) {
    const written = writeSync(3, frame, offset, frame.length - offset);
    if (written <= 0) throw new Error("Tool result channel made no progress");
    offset += written;
  }
};

try {
  const bundleBytes = await readFile(bundlePath);
  const executeDigest = createHash("sha256").update(bundleBytes).digest("hex");
  if (executeDigest !== request.seal.execute_digest) {
    throw new TypeError("executable bytes do not match the sealed digest");
  }
  // This import is intentionally the first evaluation of customer code. The Environment runner is
  // spawned only after Brain has durably journaled this operation's call intent, and the exact
  // bytes are verified above before any customer top-level code can run.
  const loaded = await import(pathToFileURL(bundlePath).href);
  const tool = loaded.default;
  if (tool === null || typeof tool !== "object" || tool.kind !== "brain.tool-runtime") {
    throw new TypeError("bundle default export is not a Tool runtime");
  }
  if (
    tool.name !== request.seal.name
    || tool.contractDigest !== request.seal.contract_digest
  ) {
    throw new TypeError("bundle Tool runtime does not match the sealed contract");
  }
  const handler = request.phase === "setup" ? tool.setup : tool.execute;
  if (typeof handler !== "function") throw new TypeError(`bundle Tool has no ${request.phase} handler`);
  const required = Array.isArray(tool.requiredEnv) ? tool.requiredEnv : [];
  if (JSON.stringify(required) !== JSON.stringify(request.seal.required_env)) {
    throw new TypeError("bundle required environment names do not match the execution seal");
  }
  for (const name of required) {
    if (process.env[name] === undefined) throw new Error(`required environment variable ${name} is unavailable`);
  }
  if (request.input === null || typeof request.input !== "object" || !("input" in request.input) || !("options" in request.input)) {
    throw new TypeError("tool input envelope is malformed");
  }
  const input = request.phase === "setup" ? undefined : request.input.input;
  const value = await handler(input, {
    signal: abort.signal,
    operationId: request.operation_id,
    sessionId: request.session_id,
    workspace: request.workspace,
    deadlineMs: request.deadline_ms,
    options: request.input.options,
  });
  const normalizedOutput = value === undefined ? null : value;
  JSON.stringify(normalizedOutput);
  writeResult(request.max_output_bytes, { ok: true, output: normalizedOutput }, normalizedOutput);
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`${message}\n`);
  if (!resultFrameStarted) {
    try {
      writeResult(request.max_output_bytes, { ok: false, error: message }, { error: message });
    } catch (writeError) {
      process.stderr.write(`could not send tool result: ${String(writeError)}\n`);
    }
  }
  process.exitCode = 1;
}
