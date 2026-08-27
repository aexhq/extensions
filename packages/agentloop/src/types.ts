export interface ContextEnvelope {
  protocolVersion: "agentloop/v1";
  items: unknown[];
  state?: unknown;
}

export type Observation =
  | { type: "session_started" }
  | { type: "user_message"; content: unknown }
  | { type: "model_completed"; response: unknown }
  | { type: "tools_completed"; results: unknown[] }
  | { type: "emitted"; event: unknown }
  | { type: "cancelled" };

export interface Presentation {
  bytes: Uint8Array;
  digest: string;
}

export interface RuntimeEnvelope {
  logicalTimeMs: bigint;
  deterministicSeed: Uint8Array;
}

export interface ActivationInput {
  context: ContextEnvelope;
  observation: Observation;
  presentation: Presentation;
  runtime: RuntimeEnvelope;
}

export interface ToolCall {
  callId: string;
  name: string;
  input: unknown;
}

export type Decision =
  | { type: "model"; request: { messages: unknown[]; response_format?: unknown; max_output_tokens?: number } }
  | { type: "tools"; calls: ToolCall[] }
  | { type: "emit"; event: unknown }
  | { type: "finish"; result?: unknown }
  | { type: "fail"; code: string; message: string; retryable?: boolean };

export interface ActivationOutput {
  context: ContextEnvelope;
  decision: Decision;
}

export interface Agentloop {
  step(input: ActivationInput): ActivationOutput;
}
