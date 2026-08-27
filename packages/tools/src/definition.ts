import { z } from "zod";

export interface ToolContext {
  signal: AbortSignal;
  workspace?: string;
  deadlineMs: number;
  grant: unknown;
}

export interface ToolDefinition {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
  output_schema?: Record<string, unknown>;
}

export interface EnvironmentTool {
  definition: ToolDefinition;
  execute(input: unknown, context: ToolContext): Promise<unknown>;
}

export function tool(input: z.ZodType, execute: (input: any, context: ToolContext) => unknown | Promise<unknown>) {
  const value = {
    definition: { name: "", description: "", input_schema: z.toJSONSchema(input) as Record<string, unknown> } as ToolDefinition,
    output: undefined as z.ZodType | undefined,
    named(name: string) { this.definition.name = name; return this; },
    describe(description: string) { this.definition.description = description; return this; },
    returns(output: z.ZodType) { this.output = output; this.definition.output_schema = z.toJSONSchema(output) as Record<string, unknown>; return this; },
    server(_module: string) { return this; },
    async execute(raw: unknown, context: ToolContext) {
      const result = await execute(input.parse(raw), context);
      return this.output === undefined ? result : this.output.parse(result);
    },
  };
  return value;
}
