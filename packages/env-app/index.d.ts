import type { EnvironmentRef, CallbacksProfile } from "@aexhq/environment";

export interface AppOptions { readonly id: string }
export interface AppHandle { readonly kind: "application-process" }
export declare const app: (options: AppOptions) => EnvironmentRef<"@aexhq/env-app", CallbacksProfile, AppHandle>;
