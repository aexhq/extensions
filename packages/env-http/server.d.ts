import type { ToolDefinition } from "@aexhq/brain";
import type { HttpInvocation } from "./handler.js";
export interface HttpBinding { url: string; token: string; timeoutMs: number; tools: readonly ToolDefinition[]; callbackUrl?: string }
export interface HttpEnvironment {
  handle(command: unknown, context?: unknown): Promise<{ contract: string; sequence: number; receipt: unknown }>;
  callback(headers: { authorization?: string }, body: unknown): Promise<number>;
}
export declare function createHttpEnvironment(options: {
  authorize(input: { sessionId: string; environment: string; configuration?: { binding?: string; authorization?: string } }, signal?: AbortSignal, context?: unknown): Promise<HttpBinding>;
  request?(url: string, options: { token: string; body: HttpInvocation; signal: AbortSignal }): Promise<unknown>;
  fetch?: typeof globalThis.fetch;
}): HttpEnvironment;
export declare function serveEnvironment(handle: HttpEnvironment["handle"], options: {
  token?: string; authenticate?(headers: { authorization?: string }): unknown | Promise<unknown>;
  callback?: HttpEnvironment["callback"]; host?: string; port?: number; maxBodyBytes?: number;
}): Promise<{ url: string; close(): Promise<void> }>;
