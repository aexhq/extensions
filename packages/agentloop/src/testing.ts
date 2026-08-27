import type { ActivationInput, ActivationOutput, Agentloop } from "./types.js";

export function step(agentloop: Agentloop, input: ActivationInput): ActivationOutput {
  return agentloop.step(structuredClone(input));
}
