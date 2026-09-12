import type { Browser } from "playwright";
export interface BrowserEnvironment { handle(command: unknown): Promise<{ contract: string; sequence: number; receipt: unknown }>; close(): Promise<void> }
export declare function createBrowserEnvironment(options: { profiles: Record<string, () => Promise<Browser>> }): BrowserEnvironment;
export declare function serveEnvironment(handle: BrowserEnvironment["handle"], options: { token: string; host?: string; port?: number; maxBodyBytes?: number }): Promise<{ url: string; close(): Promise<void> }>;
