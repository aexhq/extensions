import type { ComponentExtension } from "@aexhq/brain";
export interface AnthropicOptions { baseUrl?: string; allowHttp?: boolean }
export declare function anthropic(options?: AnthropicOptions): ComponentExtension<"model">;
