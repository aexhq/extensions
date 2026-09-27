import { z } from "zod";

const identifier = z.string().regex(/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u);
export const definition = z.strictObject({ name: identifier, description: z.string(),
  inputSchema: z.record(z.string(), z.json()), outputSchema: z.record(z.string(), z.json()).optional() });
export const legacyDescriptor = z.strictObject({ type: z.literal("http_tool"), definition,
  configuration: z.strictObject({}).optional() });
export const applicationDescriptor = z.strictObject({ type: z.literal("application_tool"), definition,
  options: z.json(), configuration: z.strictObject({}).optional() });
export const descriptor = z.discriminatedUnion("type", [legacyDescriptor, applicationDescriptor]);
const legacyInvocation = z.strictObject({ contract: z.literal("http-tool/v1"), sessionId: identifier,
  environment: identifier, sequence: z.number().int().positive().safe(), tool: definition,
  input: z.json(), deadlineAtMs: z.number().int().positive().safe() });
export const invocation = z.discriminatedUnion("contract", [legacyInvocation, legacyInvocation.extend({
  contract: z.literal("http-tool/v2"), options: z.json(),
  callback: z.strictObject({ url: z.url(), token: z.string().min(1), methods: z.array(z.enum(["result", "returned", "finish"])) }),
})]);
export const completion = z.strictObject({ type: z.literal("completed") });
export const response = z.discriminatedUnion("type", [
  z.strictObject({ type: z.literal("success"), value: z.json() }),
  z.strictObject({ type: z.literal("failure"), code: identifier, message: z.string().max(4096) }),
]);
