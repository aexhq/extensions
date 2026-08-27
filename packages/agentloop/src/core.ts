import type { Agentloop } from "./types.js";

export function defineAgentloop(agentloop: Agentloop): Agentloop {
  if (typeof agentloop?.step !== "function") {
    throw new TypeError("defineAgentloop requires a step function");
  }
  return Object.freeze(agentloop);
}
