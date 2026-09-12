export interface LocalProfile { image: string; workspace: "read" | "write"; memoryMb?: number; pids?: number }
export interface EnvironmentHandler { handle(command: unknown): Promise<{ contract: string; sequence: number; receipt: unknown }> }
export declare function createLocalEnvironment(options: { directory: string; profiles: Record<string, LocalProfile>; docker?: string }): Promise<EnvironmentHandler>;
export declare function serveEnvironment(handle: EnvironmentHandler["handle"], options: { token: string; host?: string; port?: number; maxBodyBytes?: number }): Promise<{ url: string; close(): Promise<void> }>;
