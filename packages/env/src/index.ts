import { tool } from "@aexhq/brain";
import { z } from "zod";

const reference = z.strictObject({ name: z.string().min(1), sequence: z.number().int().positive().safe() });
const request = z.discriminatedUnion("operation", [
  z.strictObject({ operation: z.literal("list") }),
  z.strictObject({ operation: z.literal("get"), environment: reference }),
  z.strictObject({ operation: z.literal("create"), template: z.string().min(1), name: z.string().min(1), configuration: z.json() }),
  z.strictObject({ operation: z.literal("update"), environment: reference, configuration: z.json() }),
  z.strictObject({ operation: z.literal("setup"), environment: reference }),
  z.strictObject({ operation: z.literal("delete"), environment: reference }),
  z.strictObject({ operation: z.literal("call"), environment: reference, method: z.string().min(1), input: z.json() }),
]);
const input = z.strictObject({
  operation: z.enum(["list", "get", "create", "update", "setup", "delete", "call"]),
  environment: reference.optional(), template: z.string().min(1).optional(), name: z.string().min(1).optional(),
  configuration: z.json().optional(), method: z.string().min(1).optional(), input: z.json().optional(),
}).pipe(request);

export const env = tool({
  name: "env",
  description: "Read or manage authorized Environments. List first for current references, recorded state and supported methods. Get reads cached state; call invokes an Environment-defined method such as inspect or restart. Never assume an uncertain operation did not run.",
  input,
  async run(request, context) {
    const environments = context.environments;
    switch (request.operation) {
      case "list": return context.finish(await environments.list());
      case "get": return context.finish(await environments.get(request.environment));
      case "create": return context.finish(await environments.create(request.template, request.name, request.configuration));
      case "update": return context.finish(await environments.update(request.environment, request.configuration));
      case "setup": return context.finish(await environments.setup(request.environment));
      case "delete": return context.finish(await environments.delete(request.environment));
      case "call": return context.finish(await environments.call(request.environment, request.method, request.input));
    }
  },
});
