import type { PlacedAgentloop, Environment } from "@aexhq/brain";

export interface PiOptions {
  readonly env: Environment;
  readonly environmentSelection?: "hidden" | "model";
  readonly placements?: Readonly<Record<string, string>>;
  /** Model context window in tokens the compaction budget is measured against. Default 200000. */
  readonly contextWindow?: number;
  /** Compact when the estimated context exceeds contextWindow - reserveTokens. pi default 16384. */
  readonly reserveTokens?: number;
  /** Approximate tokens of recent conversation kept verbatim through a compaction. pi default 20000. */
  readonly keepRecentTokens?: number;
  /** Disable automatic compaction entirely. Default true (enabled). */
  readonly compaction?: boolean;
}

export declare const pi: (options: PiOptions) => PlacedAgentloop;
