import type { PlacedAgentloop, Environment } from "@aexhq/brain";

export interface CodexOptions {
  readonly env: Environment;
  readonly environmentSelection?: "hidden" | "model";
  readonly placements?: Readonly<Record<string, string>>;
  /** Model context window in tokens; compaction triggers at 90% of it. Default 200000. */
  readonly contextWindow?: number;
  /** Disable automatic compaction entirely. Default true (enabled). */
  readonly compaction?: boolean;
  /** Hosted JSON Schema 2020-12 validation. Corrections run without tools in the same turn. */
  readonly output?: { readonly schema: boolean | Readonly<Record<string, unknown>>; readonly maxCorrections?: number };
}

export declare const codex: (options: CodexOptions) => PlacedAgentloop;
