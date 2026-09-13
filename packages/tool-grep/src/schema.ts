import { z } from "zod";

export const inputSchema = z.object({ pattern: z.string().min(1), path: z.string().default("."), limit: z.number().int().positive().max(10_000).default(1_000) });
export const outputSchema = z.object({ matches: z.array(z.string()), truncated: z.boolean() });
