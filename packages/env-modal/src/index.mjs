import { environment } from "@aexhq/brain";
import { z } from "zod";

export const modal = environment({
  options: z.strictObject({ url: z.url(), token: z.string().min(1), profile: z.string().min(1),
    lifetimeMs: z.number().int().positive().max(86_400_000), authorization: z.string().min(1).optional() }),
  url: ({ url }) => url,
  credential: ({ token }) => token,
  configure: ({ profile, lifetimeMs, authorization }) => ({ profile, lifetimeMs, ...(authorization === undefined ? {} : { authorization }) }),
});
