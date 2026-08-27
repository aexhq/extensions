import { defineAgentloop } from "../../dist/index.js";

export default defineAgentloop({
  step(input) {
    const activations = Number(input.context.state?.activations ?? 0) + 1;
    const context = { ...input.context, state: { activations, last: input.observation.type } };
    if (input.observation.type === "user_message") {
      const scenario = input.observation.content?.scenario ?? "finish";
      if (scenario === "model") {
        return { context, decision: { type: "model", request: { messages: [{ role: "user", content: "diagnostic" }] } } };
      }
      if (scenario === "tools") {
        return { context, decision: { type: "tools", calls: [
          { callId: "diagnostic-a", name: "diagnostic", input: { index: 0 } },
          { callId: "diagnostic-b", name: "diagnostic", input: { index: 1 } },
        ] } };
      }
      if (scenario === "emit") {
        return { context, decision: { type: "emit", event: { scenario, activations } } };
      }
      if (scenario === "fail") {
        return { context, decision: { type: "fail", code: "diagnostic_failure", message: "requested failure" } };
      }
    }
    return { context, decision: { type: "finish", result: { observation: input.observation.type, activations } } };
  },
});
