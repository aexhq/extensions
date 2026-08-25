import type { ComponentExtension } from "@aexhq/brain";

export interface AwsMicrovmOptions {
  readonly region?: string;
  readonly idleSeconds?: number;
  readonly maximumSeconds?: number;
}

export type AwsMicrovmEnvironment = ComponentExtension<
  "environment",
  {
    readonly driver: "aws-microvm";
    readonly configuration: {
      readonly region?: string;
      readonly idle_seconds?: number;
      readonly maximum_seconds?: number;
    };
  }
>;

export declare function awsMicrovm(options?: AwsMicrovmOptions): AwsMicrovmEnvironment;
