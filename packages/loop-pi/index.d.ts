import type { AgentloopSelection } from "@aexhq/brain";
export interface PiOptions { contextWindow?: number; maxTokens?: number; compaction?: { enabled?: boolean; reserveTokens?: number; keepRecentTokens?: number }; }
export declare function pi(configuration?: PiOptions): AgentloopSelection;
