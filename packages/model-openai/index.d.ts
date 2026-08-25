import type { ComponentExtension } from "@aexhq/brain";
export interface OpenAiOptions { baseUrl?: string; allowHttp?: boolean }
export declare function openai(options?: OpenAiOptions): ComponentExtension<"model">;
