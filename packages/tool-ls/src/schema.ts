import { z } from "zod";

export const inputSchema = z.object({ path: z.string().default("."), limit: z.number().int().positive().max(10_000).default(1_000) });
export const outputSchema = z.object({ entries: z.array(z.object({ name: z.string(), kind: z.enum(["file", "dir"]) })), truncated: z.boolean() });
