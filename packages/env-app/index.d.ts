export interface EnvironmentToolContext {
  signal: AbortSignal;
  grant: unknown;
  environmentId: string;
  sessionId: string;
  attachmentId: string;
}

export type EnvironmentTool = (input: unknown, context: EnvironmentToolContext) => unknown | Promise<unknown>;

export interface EnvironmentOptions {
  tools?: Record<string, EnvironmentTool>;
  token?: string;
  receipts?: Map<string, unknown>;
  maxReceipts?: number;
  setup?(input: unknown): unknown | Promise<unknown>;
  attach?(input: unknown): unknown | Promise<unknown>;
  call?(input: unknown): unknown | Promise<unknown>;
  cancel?(input: unknown): unknown | Promise<unknown>;
  detach?(input: unknown): unknown | Promise<unknown>;
  teardown?(input: unknown): unknown | Promise<unknown>;
}

export declare function createEnvironment(options?: EnvironmentOptions): (command: unknown) => Promise<unknown>;
export declare function serveEnvironment(options?: EnvironmentOptions): {
  server: import("node:http").Server;
  listen(port?: number, host?: string): Promise<unknown>;
  close(): Promise<void>;
};
