import type { AgentloopSelection } from "@aexhq/brain";
export interface CodexOptions { contextWindow?: number; compactAtTokens?: number; }
export declare function codex(configuration?: CodexOptions): AgentloopSelection;
