import type { PlacedTool, ToolDefinition } from "@aexhq/brain";
export interface HttpInvocation {
  contract: "http-tool/v1"; sessionId: string; environment: string; sequence: number;
  tool: ToolDefinition; input: unknown; deadlineAtMs: number;
}
export declare function createToolHandler(options: {
  tools: readonly PlacedTool[];
  authorize(request: Request, invocation: HttpInvocation): void | Promise<void>;
  maxBodyBytes?: number;
}): (request: Request) => Promise<Response>;
