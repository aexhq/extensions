import { z } from "zod";

const identifier = z.string().regex(/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u);
export const definition = z.strictObject({ name: identifier, description: z.string(),
  inputSchema: z.record(z.string(), z.json()), outputSchema: z.record(z.string(), z.json()).optional() });
export const descriptor = z.strictObject({ type: z.literal("http_tool"), definition,
  configuration: z.strictObject({}).optional() });
export const invocation = z.strictObject({ contract: z.literal("http-tool/v1"), sessionId: identifier,
  environment: identifier, sequence: z.number().int().positive().safe(), tool: definition,
  input: z.json(), deadlineAtMs: z.number().int().positive().safe() });
export const response = z.discriminatedUnion("type", [
  z.strictObject({ type: z.literal("success"), value: z.json() }),
  z.strictObject({ type: z.literal("failure"), code: identifier, message: z.string().max(4096) }),
]);
