import { inspect, listChildren, manage, send, spawn, wait } from "aex:tool/children@1.0.0";

export function invoke(request) {
  const input = JSON.parse(request.inputJson);
  let value;
  switch (input.action) {
    case "spawn_agent":
      value = JSON.parse(spawn(JSON.stringify({
        prompt: required(input, "message"),
        name: required(input, "task_name"),
        ...(input.fork_turns === undefined ? {} : { fork_turns: input.fork_turns }),
      })));
      break;
    case "send_message":
    case "follow_up":
      value = JSON.parse(send(required(input, "child_id"), JSON.stringify(required(input, "message"))));
      break;
    case "wait":
      value = JSON.parse(wait(required(input, "child_id"), BigInt(input.timeout_ms ?? 30_000)));
      break;
    case "peek":
      value = JSON.parse(inspect(required(input, "child_id")));
      break;
    case "list_children": {
      const page = listChildren(input.cursor, input.limit ?? 20);
      value = {
        data: JSON.parse(page.itemsJson),
        has_more: page.nextCursor !== undefined,
        ...(page.nextCursor === undefined ? {} : { next_cursor: page.nextCursor }),
      };
      break;
    }
    case "interrupt_agent":
      value = JSON.parse(manage(required(input, "child_id"), "cancel"));
      break;
    case "end_agent":
      value = JSON.parse(manage(required(input, "child_id"), "end"));
      break;
    default:
      throw new TypeError(`unknown subagents action ${JSON.stringify(input.action)}`);
  }
  const encoded = JSON.stringify(value);
  return { valueJson: encoded, content: encoded, isError: false };
}

function required(input, field) {
  const value = input[field];
  if (typeof value !== "string" || value.length === 0) {
    throw new TypeError(`subagents ${field} is required`);
  }
  return value;
}
