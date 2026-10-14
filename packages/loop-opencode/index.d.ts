import type { AgentloopSelection } from "@aexhq/brain";
export interface OpencodeOptions { contextWindow?: number; maxTokens?: number; compaction?: { auto?: boolean; buffer?: number; keep?: { tokens: number } }; }
export declare function opencode(configuration?: OpencodeOptions): AgentloopSelection;
