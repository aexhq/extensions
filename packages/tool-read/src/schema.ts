import { z } from "zod";

export const inputSchema = z.object({
  path: z.string().min(1),
  offset: z.number().int().nonnegative().default(0),
  limit: z.number().int().positive().max(1024 * 1024).default(256 * 1024),
});
export const outputSchema = z.object({ content: z.string(), bytes: z.number().int().nonnegative(), truncated: z.boolean() });
