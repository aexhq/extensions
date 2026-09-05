import { z } from "zod";

export const bashInput = z.object({ command: z.string().min(1) });
export const bashOutput = z.object({ exit_code: z.number().int(), stdout: z.string(), stderr: z.string() });

export const editInput = z.object({ path: z.string().min(1), old_text: z.string().min(1), new_text: z.string() });
export const editOutput = z.object({ path: z.string(), replacements: z.literal(1) });

export const globInput = z.object({ pattern: z.string().min(1), limit: z.number().int().positive().max(10_000).default(1_000) });
export const globOutput = z.object({ paths: z.array(z.string()), truncated: z.boolean() });

export const grepInput = z.object({ pattern: z.string().min(1), path: z.string().default("."), limit: z.number().int().positive().max(10_000).default(1_000) });
export const grepOutput = z.object({ matches: z.array(z.string()), truncated: z.boolean() });

export const lsInput = z.object({ path: z.string().default("."), limit: z.number().int().positive().max(10_000).default(1_000) });
export const lsOutput = z.object({ entries: z.array(z.object({ name: z.string(), kind: z.enum(["file", "dir"]) })), truncated: z.boolean() });

export const readInput = z.object({
  path: z.string().min(1),
  offset: z.number().int().nonnegative().default(0),
  limit: z.number().int().positive().max(1024 * 1024).default(256 * 1024),
});
export const readOutput = z.object({ content: z.string(), bytes: z.number().int().nonnegative(), truncated: z.boolean() });

const todoItem = z.object({ text: z.string().min(1), done: z.boolean().default(false) });
export const todoInput = z.discriminatedUnion("action", [
  z.object({ action: z.literal("get") }),
  z.object({ action: z.literal("set"), items: z.array(todoItem).max(200) }),
]);
export const todoOutput = z.object({ items: z.array(todoItem) });

export const writeInput = z.object({ path: z.string().min(1), content: z.string() });
export const writeOutput = z.object({ path: z.string(), bytes: z.number().int().nonnegative() });
