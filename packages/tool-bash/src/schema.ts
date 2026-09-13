import { z } from "zod";

export const inputSchema = z.object({ command: z.string().min(1) });
export const outputSchema = z.object({ exit_code: z.number().int(), stdout: z.string(), stderr: z.string() });
