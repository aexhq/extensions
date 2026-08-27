export interface AwsMicrovmOptions {
  readonly id: string;
  readonly region?: string;
  readonly idleSeconds?: number;
  readonly maximumSeconds?: number;
  readonly lifecyclePolicy?: "session" | "shared" | "external";
}

export interface AwsMicrovmEnvironment {
  readonly environment_id: string;
  readonly configuration: {
    readonly driver: "aws-microvm";
    readonly region?: string;
    readonly idle_seconds?: number;
    readonly maximum_seconds?: number;
  };
  readonly lifecycle_policy: "session" | "shared" | "external";
}

export declare function awsMicrovm(options: AwsMicrovmOptions): AwsMicrovmEnvironment;
