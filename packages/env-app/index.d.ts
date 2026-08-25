import type { ComponentExtension } from "@aexhq/brain";
import type { ToolDefinition } from "@aexhq/brain/session";

export interface AppOptions {
  readonly id: string;
}

export type AppEnvironment = ComponentExtension<
  "environment",
  { readonly driver: "customer"; readonly configuration: { readonly registration: string } }
>;

export declare function app(options: AppOptions): AppEnvironment;

export declare function callback(
  definition: ToolDefinition,
  registration?: string,
): ComponentExtension<
  "tool",
  {
    readonly definition: ToolDefinition;
    readonly descriptor: {
      readonly registration: string;
      readonly name: string;
      readonly contract_digest: string;
    };
  }
>;
