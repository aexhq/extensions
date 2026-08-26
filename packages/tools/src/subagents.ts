import { officialTool } from "@aexhq/brain/internal";
import { z } from "zod";

const childId = z.string().min(1).max(128).regex(/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/);
const message = z.string().min(1).max(192 * 1024);
const forkTurns = z.union([
  z.literal("all"),
  z.literal("none"),
  z.string().max(10).regex(/^[1-9][0-9]*$/),
]);

// Spawning reaches parent and child session data, which is why this is a builtin a customer
// turns on rather than a sandboxed extension: `brain.subagents` resolves inside Brain and never
// crosses the component host. The capability name is fixed here so it is never a caller's choice.
const subagents = officialTool({
  name: "subagents",
  description: "Create and explicitly interact with durable direct child sessions.",
  input: z.object({
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
  capability: "brain.subagents",
});

export default subagents;
