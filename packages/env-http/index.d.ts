import type { Environment, PlacedTool } from "@aexhq/brain";
export declare function http(options: { name: string; url: string; token?: string; binding: string }): Environment;
export declare function httpTool<Input, Output>(tool: PlacedTool<Input, Output>, options: { env: Environment }): PlacedTool<Input, Output>;
