import type { Environment, EnvironmentGrant, EnvironmentTemplate, PlacedTool } from "@aexhq/brain";
export declare function http(options: { name: string; environments?: readonly EnvironmentGrant[]; template?: EnvironmentTemplate; url: string; token?: string; binding: string }): Environment;
export declare function httpTool<Input, Output>(tool: PlacedTool<Input, Output>, options: { env: Environment }): PlacedTool<Input, Output>;
export declare function application(options: { name: string; url: string; endpoint: string; credential: string; timeoutMs?: number; environments?: readonly EnvironmentGrant[] }): Environment;
export declare function applicationTool<Input, Output>(tool: PlacedTool<Input, Output>, options: { env: Environment }): PlacedTool<Input, Output>;
