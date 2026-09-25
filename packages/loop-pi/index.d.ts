import type { PlacedAgentloop, Environment, EnvironmentGrant } from "@aexhq/brain";

export interface PiOptions {
  readonly env: Environment;
  readonly environments?: readonly EnvironmentGrant[];
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
  /** Hosted JSON Schema 2020-12 validation. Corrections run without tools in the same turn. */
  readonly output?: { readonly schema: boolean | Readonly<Record<string, unknown>>; readonly maxCorrections?: number };
}

export declare const pi: (options: PiOptions) => PlacedAgentloop;
