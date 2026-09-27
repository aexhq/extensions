import type { PlacedTool, ToolDefinition } from "@aexhq/brain";
interface InvocationFields {
  sessionId: string; environment: string; sequence: number;
  tool: ToolDefinition; input: unknown; deadlineAtMs: number;
}
export type HttpInvocation = InvocationFields & ({ contract: "http-tool/v1" } | {
  contract: "http-tool/v2"; options: unknown;
  callback: { url: string; token: string; methods: readonly ("result" | "returned" | "finish")[] };
});
export declare function createToolHandler(options: {
  tools: readonly PlacedTool[];
  authorize(request: Request, invocation: HttpInvocation): void | Promise<void>;
  maxBodyBytes?: number;
  callbackRequest?(url: string, options: { token: string; body: unknown; signal: AbortSignal }): Promise<unknown>;
}): (request: Request) => Promise<Response>;
