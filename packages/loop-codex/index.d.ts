import type { Agentloop } from "@aexhq/brain";

export interface CodexOptions {
  /** Model context window in tokens; compaction triggers at 90% of it. Default 200000. */
  readonly contextWindow?: number;
  /** Disable automatic compaction entirely. Default true (enabled). */
  readonly compaction?: boolean;
}

export declare const codex: (options?: CodexOptions) => Agentloop;
