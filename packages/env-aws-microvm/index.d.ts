import type { ComputerProfile, EnvironmentRef } from "@aexhq/environment";

export interface AwsMicrovmOptions { readonly region?: string }
export interface AwsMicrovmStatus {
  readonly state: string;
  readonly generation?: string;
}
export interface AwsMicrovmHandle {
  status(): Promise<AwsMicrovmStatus>;
  readonly files: {
    list(path?: string): Promise<unknown>;
    read(path: string): Promise<Uint8Array>;
    upload(path: string, content: string | Uint8Array, options?: { readonly overwrite?: boolean }): Promise<unknown>;
  };
}
export declare const awsMicrovm: (
  options?: AwsMicrovmOptions,
) => EnvironmentRef<"@aexhq/env-aws-microvm", ComputerProfile, AwsMicrovmHandle>;
