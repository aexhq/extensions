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
  readonly bundleBase64: string;
}

export type OfficialTool = ComponentExtension<"tool", OfficialToolConfig>;

export declare const bash: () => OfficialTool;
export declare const edit: () => OfficialTool;
export declare const glob: () => OfficialTool;
export declare const grep: () => OfficialTool;
export declare const ls: () => OfficialTool;
export declare const read: () => OfficialTool;
export declare const todo: () => OfficialTool;
export declare const write: () => OfficialTool;
