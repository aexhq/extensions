import type { Browser } from "playwright";
import type { HostToolCall } from "@aexhq/brain";
export interface BrowserEnvironment { handle(command: unknown): Promise<{ contract: string; sequence: number; receipt: unknown }>; close(): Promise<void> }
export declare function createBrowserEnvironment(options: { profiles: Record<string, () => Promise<Browser>>;
  publishMedia?: (media: { bytes: Uint8Array; mediaType: "image/png"; index: number }, context: Pick<HostToolCall, "sessionId" | "sequence" | "signal">) => string | Promise<string> }): BrowserEnvironment;
export declare function serveEnvironment(handle: BrowserEnvironment["handle"], options: { token: string; host?: string; port?: number; maxBodyBytes?: number }): Promise<{ url: string; close(): Promise<void> }>;
