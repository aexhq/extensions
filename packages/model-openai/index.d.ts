import type { ComponentExtension } from "@aexhq/brain";
export interface OpenAiOptions {
  baseUrl?: string;
  allowHttp?: boolean;
  outputTokenParameter?: "max_tokens" | "max_completion_tokens";
}
export declare function openai(options?: OpenAiOptions): ComponentExtension<"model">;
