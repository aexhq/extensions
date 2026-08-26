import type { ComponentExtension } from "@aexhq/brain";

export interface OfficialToolConfig {
  readonly definition: {
    readonly name: string;
    readonly description?: string;
    readonly input_schema: Record<string, unknown>;
    readonly output_schema?: Record<string, unknown>;
    readonly contract_digest: string;
  };
  readonly descriptor: Readonly<Record<string, unknown>>;
}

export type OfficialTool = ComponentExtension<"tool", OfficialToolConfig>;

export interface ChildToolConfig {
  readonly definition: OfficialToolConfig["definition"];
}

export type ChildTool = ComponentExtension<"tool", ChildToolConfig>;

export declare const bash: () => OfficialTool;
export declare const edit: () => OfficialTool;
export declare const glob: () => OfficialTool;
export declare const grep: () => OfficialTool;
export declare const ls: () => OfficialTool;
export declare const read: () => OfficialTool;
export declare const todo: () => OfficialTool;
export declare const write: () => OfficialTool;
export declare const subagents: () => ChildTool;
export declare const task: () => ChildTool;
