import type { Environment, EnvironmentGrant, EnvironmentTemplate } from "@aexhq/brain";
export declare function local(options: { name: string; environments?: readonly EnvironmentGrant[]; template?: EnvironmentTemplate; url: string; token: string; profile: string }): Environment;
