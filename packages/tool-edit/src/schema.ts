import { z } from "zod";

export const inputSchema = z.object({ path: z.string().min(1), old_text: z.string().min(1), new_text: z.string() });
export const outputSchema = z.object({ path: z.string(), replacements: z.literal(1) });
