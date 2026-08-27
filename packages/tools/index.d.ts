export interface RemoteTool {
  readonly definition: {
    readonly name: string;
    readonly description: string;
    readonly input_schema: Record<string, unknown>;
    readonly output_schema?: Record<string, unknown>;
  };
  readonly remoteToolId: string;
}

export declare const definitions: Readonly<Record<string, RemoteTool>>;
export declare const handlers: Readonly<Record<string, (input: unknown, context: unknown) => Promise<unknown>>>;
export declare const bash: () => RemoteTool;
export declare const edit: () => RemoteTool;
export declare const glob: () => RemoteTool;
export declare const grep: () => RemoteTool;
export declare const ls: () => RemoteTool;
export declare const read: () => RemoteTool;
export declare const todo: () => RemoteTool;
export declare const write: () => RemoteTool;
