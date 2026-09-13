import { z } from "zod";

export const inputSchema = z.object({ path: z.string().min(1), content: z.string() });
export const outputSchema = z.object({ path: z.string(), bytes: z.number().int().nonnegative() });
