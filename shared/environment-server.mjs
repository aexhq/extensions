import { createServer } from "node:http";
import { once } from "node:events";
import { z } from "zod";

export const identifier = z.string().regex(/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u);
const sequence = z.number().int().positive().safe();
const callback = z.strictObject({ url: z.url(), token: z.string().min(1), methods: z.array(z.string()) });
const command = z.strictObject({
  contract: z.literal("environment/v1"),
  operation: z.strictObject({ session_id: identifier, environment: identifier, sequence,
    request: z.discriminatedUnion("type", [
      z.strictObject({ type: z.literal("setup"), configuration: z.unknown() }),
      z.strictObject({ type: z.literal("execute"), implementation: z.unknown(), input: z.unknown(),
        deadline_ms: sequence.max(2_147_483_647), callback: callback.optional() }),
      z.strictObject({ type: z.literal("call"), name: identifier, input: z.unknown() }),
      z.strictObject({ type: z.literal("cancel"), target_sequence: sequence }),
      z.strictObject({ type: z.literal("detach") }), z.strictObject({ type: z.literal("teardown") }),
    ]),
  }),
});

export class EnvironmentError extends Error {
  constructor(code, message) { super(message); this.code = code; }
}
export const fail = (code, message) => { throw new EnvironmentError(code, message); };
export const accepted = () => ({ type: "accepted" });
export const result = output => ({ type: "result", output });
export const failure = (code, message, details) => ({ type: "failure", code,
  message: String(message).slice(0, 4096), retryable: false, ...(details === undefined ? {} : { details }) });
export const unknown = message => ({ type: "unknown", message: String(message).slice(0, 4096) });
export const bindingKey = op => `${op.session_id}/${op.environment}`;

export function environmentHandler(handle) {
  return async raw => {
    const { operation } = command.parse(raw);
    let receipt;
    try { receipt = await handle(operation); }
    catch (error) {
      receipt = failure(error instanceof EnvironmentError ? error.code : "environment_failed", error.message ?? error);
    }
    return { contract: "environment/v1", sequence: operation.sequence, receipt };
  };
}

export async function serveEnvironment(handle, { token, host = "127.0.0.1", port = 0, maxBodyBytes = 16 * 1024 * 1024 } = {}) {
  z.string().min(1).parse(token);
  z.number().int().positive().safe().parse(maxBodyBytes);
  const server = createServer(async (request, response) => {
    if (request.headers.authorization !== `Bearer ${token}`) { request.resume(); response.writeHead(401).end(); return; }
    if (request.method !== "POST" || request.url !== "/v1/operations") { request.resume(); response.writeHead(404).end(); return; }
    try {
      let bytes = 0;
      const chunks = [];
      for await (const chunk of request) {
        bytes += chunk.length;
        if (bytes > maxBodyBytes) { response.writeHead(413).end(); return; }
        chunks.push(chunk);
      }
      const body = await handle(JSON.parse(Buffer.concat(chunks).toString("utf8")));
      response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(body));
    } catch (error) {
      response.writeHead(error instanceof SyntaxError || error instanceof z.ZodError ? 400 : 500,
        { "content-type": "application/json" }).end(JSON.stringify({ error: String(error.message ?? error).slice(0, 4096) }));
    }
  });
  server.listen(port, host);
  await once(server, "listening");
  return { url: `http://${host.includes(":") ? `[${host}]` : host}:${server.address().port}`,
    close: () => new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve())) };
}
