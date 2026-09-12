import type { Client, ClientOptions, Transport } from "@modelcontextprotocol/client";
import type { Environment, HostToolCall, PlacedTool } from "@aexhq/brain";
export declare function connectMcp(transport: Transport, options?: ClientOptions & { name?: string; version?: string }): Promise<Client>;
export interface McpPresentation { content: unknown[]; structuredContent?: unknown; evidenceSequence: number }
export declare function mcpTools(options: { client: Client; names: string[]; env: Environment; prefix?: string;
  publishMedia?: (media: { bytes: Uint8Array; mediaType: "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "application/pdf"; index: number }, context: HostToolCall) => string | Promise<string>;
  project?: (output: McpPresentation, toolName: string) => unknown | Promise<unknown> }): Promise<PlacedTool[]>;
