export interface PiOptions {
  readonly instructions?: string;
  readonly temperature?: number;
  readonly reasoningEffort?: "low" | "medium" | "high";
}

export interface PiExtension {
  readonly source: string;
  readonly sha256: string;
  readonly toolchain: string;
}

export declare function pi(options?: PiOptions): PiExtension;
