import type { ComponentExtension } from "@aexhq/brain";

export interface AwsMicrovmOptions {
  readonly region?: string;
}

export type AwsMicrovmEnvironment = ComponentExtension<
  "environment",
  {
    readonly driver: "aws-microvm";
    readonly configuration: { readonly region?: string };
  }
>;

export declare function awsMicrovm(options?: AwsMicrovmOptions): AwsMicrovmEnvironment;
