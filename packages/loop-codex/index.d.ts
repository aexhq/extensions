export interface CodexOptions {
  readonly instructions?: string;
  readonly temperature?: number;
  readonly reasoningEffort?: "low" | "medium" | "high";
}

import type { ComponentExtension } from "@aexhq/brain";

export type CodexExtension = ComponentExtension<"agentloop", CodexOptions>;

export declare function codex(options?: CodexOptions): CodexExtension;
