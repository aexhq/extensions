import type { Environment, PlacedTool } from "@aexhq/brain";
export declare function browser(options: { name: string; url: string; token: string; profile: string }): Environment;
export declare function navigate(options: { env: Environment }): PlacedTool<{ url: string }>;
export declare function inspect(options: { env: Environment }): PlacedTool<Record<string, never>>;
export declare function click(options: { env: Environment }): PlacedTool<{ selector: string }>;
export declare function fill(options: { env: Environment }): PlacedTool<{ selector: string; value: string }>;
export declare function screenshot(options: { env: Environment }): PlacedTool<Record<string, never>>;
export declare function browserTools(options: { env: Environment }): PlacedTool[];
