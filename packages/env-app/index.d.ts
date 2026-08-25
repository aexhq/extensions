import type { ComponentExtension } from "@aexhq/brain";

export interface AppOptions {
  readonly id: string;
}

export type AppEnvironment = ComponentExtension<
  "environment",
  { readonly driver: "customer"; readonly configuration: { readonly registration: string } }
>;

export declare function app(options: AppOptions): AppEnvironment;
