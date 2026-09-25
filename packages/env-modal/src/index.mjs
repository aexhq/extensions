import { environment } from "@aexhq/brain";
import { z } from "zod";

export const modal = environment({
  methods: {
    inspect: { effect: "none", description: "Inspect the original Modal Sandbox and its lifetime without allocating a replacement.", input_schema: z.toJSONSchema(z.strictObject({})) },
    terminate: { effect: "none", description: "Terminate this Modal Sandbox and all its active invocations.", input_schema: z.toJSONSchema(z.strictObject({ sequence: z.number().int().positive().optional() })) },
  },
  options: z.strictObject({ url: z.url(), token: z.string().min(1).optional(), profile: z.string().min(1),
    lifetimeMs: z.number().int().positive().max(86_400_000), authorization: z.string().min(1).optional() }),
  url: ({ url }) => url,
  credential: ({ token }) => token,
  configure: ({ profile, lifetimeMs, authorization }) => ({ profile, lifetimeMs, ...(authorization === undefined ? {} : { authorization }) }),
});
