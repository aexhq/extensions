export interface CodexOptions {
  readonly instructions?: string;
  readonly temperature?: number;
  readonly reasoningEffort?: "low" | "medium" | "high";
}

export interface CodexExtension {
  readonly source: string;
  readonly sha256: string;
  readonly toolchain: string;
}

export declare function codex(options?: CodexOptions): CodexExtension;
