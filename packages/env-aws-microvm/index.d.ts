import type { Environment } from "@aexhq/brain";

export type AwsMicroVmOptions = {
  readonly region?: string;
  readonly idleSeconds?: number;
  readonly maximumSeconds?: number;
} & (
  | { readonly lifecycle?: "session"; readonly id?: never }
  | { readonly lifecycle: "shared" | "external"; readonly id: string }
);

export type AwsMicroVmEnvironment = Environment<"workspace">;

export declare function awsMicroVm(options?: AwsMicroVmOptions): AwsMicroVmEnvironment;
