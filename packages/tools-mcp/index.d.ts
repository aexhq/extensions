import type { Client, ClientOptions, Transport } from "@modelcontextprotocol/client";
import type { Environment, PlacedTool } from "@aexhq/brain";
export declare function connectMcp(transport: Transport, options?: ClientOptions & { name?: string; version?: string }): Promise<Client>;
export interface McpPresentation { content: unknown[]; structuredContent?: unknown; evidenceSequence: number }
export declare function mcpTools(options: { client: Client; names: string[]; env: Environment; prefix?: string;
  project?: (output: McpPresentation, toolName: string) => unknown | Promise<unknown> }): Promise<PlacedTool[]>;
