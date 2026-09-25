import type { Environment, EnvironmentGrant, EnvironmentTemplate, PlacedTool } from "@aexhq/brain";
export declare function http(options: { name: string; environments?: readonly EnvironmentGrant[]; template?: EnvironmentTemplate; url: string; token?: string; binding: string }): Environment;
export declare function httpTool<Input, Output>(tool: PlacedTool<Input, Output>, options: { env: Environment }): PlacedTool<Input, Output>;
