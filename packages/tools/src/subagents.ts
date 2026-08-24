import { tool } from "@aexhq/sdk";
import { z } from "zod";

const input = z.object({
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
  message: z.string().min(1).max(192 * 1024).optional(),
  fork_turns: z.union([
    z.literal("all"),
    z.literal("none"),
    z.string().max(10).regex(/^[1-9][0-9]*$/u),
  ]).optional(),
  child_id: z.string().min(1).max(128).regex(/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u).optional(),
  timeout_ms: z.number().int().nonnegative().max(300_000).optional(),
  cursor: z.string().max(4096).optional(),
  limit: z.number().int().positive().max(100).optional(),
});

function required(value: string | undefined, name: string): string {
  if (value === undefined) throw new Error(`subagents ${name} is required`);
  return value;
}

const subagents = tool(input, async function subagents(request, context) {
  const baseUrl = required(process.env.SUBAGENTS_API_URL, "environment variable SUBAGENTS_API_URL").replace(/\/$/u, "");
  const token = required(process.env.SUBAGENTS_TOKEN, "environment variable SUBAGENTS_TOKEN");
  const child = request.child_id === undefined ? undefined : encodeURIComponent(request.child_id);
  let method = "GET";
  let path = `/v1/sessions/${encodeURIComponent(context.sessionId)}/children`;
  let body: Record<string, unknown> | undefined;

  switch (request.action) {
    case "spawn_agent":
      method = "POST";
      body = {
        prompt: required(request.message, "message"),
        name: required(request.task_name, "task_name"),
        ...(request.fork_turns === undefined ? {} : { fork_turns: request.fork_turns }),
      };
      break;
    case "send_message":
    case "follow_up":
      method = "POST";
      path += `/${required(child, "child_id")}/messages`;
      body = { message: required(request.message, "message") };
      break;
    case "peek":
      path += `/${required(child, "child_id")}`;
      break;
    case "wait":
      path += `/${required(child, "child_id")}/wait`;
      if (request.timeout_ms !== undefined) path += `?timeout_ms=${request.timeout_ms}`;
      break;
    case "list_children": {
      const query = new URLSearchParams();
      if (request.cursor !== undefined) query.set("cursor", request.cursor);
      if (request.limit !== undefined) query.set("limit", String(request.limit));
      if (query.size > 0) path += `?${query}`;
      break;
    }
    case "interrupt_agent":
      method = "POST";
      path += `/${required(child, "child_id")}/cancel`;
      body = {};
      break;
    case "end_agent":
      method = "POST";
      path += `/${required(child, "child_id")}/end`;
      body = {};
      break;
  }

  const response = await fetch(`${baseUrl}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      ...(body === undefined ? {} : { "content-type": "application/json" }),
      ...(method === "GET" ? {} : { "idempotency-key": context.operationId }),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: context.signal,
  });
  const text = await response.text();
  if (!response.ok) {
    let message = text;
    try {
      const failure = JSON.parse(text) as { error?: { message?: unknown } };
      if (typeof failure.error?.message === "string") message = failure.error.message;
    } catch {}
    throw new Error(message || `subagents request failed with status ${response.status}`);
  }
  return text === "" ? {} : JSON.parse(text) as unknown;
})
  .named("subagents")
  .describe("Create and explicitly interact with durable direct child sessions.")
  .returns(z.unknown())
  .needs({
    env: ["SUBAGENTS_API_URL", "SUBAGENTS_TOKEN"],
    network: [{ host: "api.aex.dev", port: 443 }],
  });

export default subagents;
