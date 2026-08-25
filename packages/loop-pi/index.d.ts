export interface PiOptions {
  readonly instructions?: string;
  readonly temperature?: number;
  readonly reasoningEffort?: "low" | "medium" | "high";
}

import type { ComponentExtension } from "@aexhq/brain";

export type PiExtension = ComponentExtension<"agentloop", PiOptions>;

export declare function pi(options?: PiOptions): PiExtension;
