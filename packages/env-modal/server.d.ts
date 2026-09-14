import type { ModalClient } from "modal";
export declare function createModalClient(options?: { tokenId?: string; tokenSecret?: string; environment?: string }): ModalClient;
export interface ModalProfile {
  image: string; commands: Record<string, string[]>; cpu: number; memoryMiB: number;
  maxLifetimeMs: number; region: string; workdir?: string; outboundDomains?: string[]; maxOutputBytes?: number;
}
export interface ModalConfiguration { profile: string; lifetimeMs: number; authorization?: string }
export interface ResourceUsage {
  sessionId: string; environment: string; authorization?: string; profile: string;
  sandboxId: string | null; unitsMs: number; terminal: boolean;
}
export interface ModalEnvironment {
  handle(command: unknown): Promise<{ contract: string; sequence: number; receipt: unknown }>;
  reconcile(): Promise<Array<{ binding: string; phase: string; error?: string }>>;
  recover(resource: { sessionId: string; environment: string; sandboxId: string }): Promise<void>;
  close(): void;
}
export declare function createModalEnvironment(options: {
  directory: string; appName: string; profiles: Record<string, ModalProfile>; client?: ModalClient;
  authorize?(binding: { sessionId: string; environment: string; configuration: ModalConfiguration }): Promise<number>;
  report?(usage: ResourceUsage): Promise<void>;
}): Promise<ModalEnvironment>;
export declare function serveEnvironment(handle: ModalEnvironment["handle"], options: {
  token: string; host?: string; port?: number; maxBodyBytes?: number;
}): Promise<{ url: string; close(): Promise<void> }>;
