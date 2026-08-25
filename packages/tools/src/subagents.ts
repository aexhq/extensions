import { tool } from "@aexhq/brain";
import { z } from "zod";

const childId = z.string().min(1).max(128).regex(/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/);
const message = z.string().min(1).max(192 * 1024);
const forkTurns = z.union([
  z.literal("all"),
  z.literal("none"),
  z.string().max(10).regex(/^[1-9][0-9]*$/),
]);

const subagents = tool(
  z.object({
    action: z.enum([
      "spawn_agent",
      "send_message",
      "follow_up",
      "wait",
      "peek",
      "list_children",
      "interrupt_agent",
      "end_agent",
    ]),
    task_name: z.string().min(1).max(128).optional(),
    message: message.optional(),
    fork_turns: forkTurns.optional(),
    child_id: childId.optional(),
    timeout_ms: z.number().int().nonnegative().max(300_000).optional(),
    cursor: z.string().max(4096).optional(),
    limit: z.number().int().positive().max(100).optional(),
  }),
  async () => {
    throw new Error("subagents runs through the Tool child-session capability");
  },
)
  .named("subagents")
  .describe("Create and explicitly interact with durable direct child sessions.")
  .returns(z.unknown())
  .server(import.meta.url);

export default subagents;
