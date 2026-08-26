// The smallest loop an author can write with `@aexhq/agentloop`: hydration replayed into one model
// round, one journal entry and an explicit finish. Compiled by the staged package's own builder,
// this is the only place its host binding and activation envelope run inside a real component.
import { defineAgentloop } from "@aexhq/agentloop";

let memory = [];

export const { activate } = defineAgentloop({
  onSessionStart(start) {
    memory = start.tail.flatMap((view) =>
      view.type === "user_message" ? [{ role: "user", content: view.content }] : [],
    );
  },
  async onMessage(ctx, message) {
    memory.push({ role: "user", content: message.content });
    const round = await ctx.model.stream({ messages: memory.slice() });
    await ctx.journal.append([
      { kind: "event", name: "smoke.round", data: { stop: round.stop_reason } },
    ]);
    await ctx.turn.finish({ text: round.content.map((block) => block.text ?? "").join("") });
  },
});
