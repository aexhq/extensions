const MAX_CONTEXT_BYTES = 131072;
const DELEGATE = "brain_delegate";

function delegationEnabled(request) {
  return request.config.harness?.configuration?.delegation === true;
}

function modelActions(request) {
  const actions = request.config.actions ?? [];
  if (!delegationEnabled(request)) return actions;
  if (actions.some(action => action.name === DELEGATE)) throw new Error("delegation cannot shadow an admitted application action");
  return [...actions, { name: DELEGATE, connection: "harness", max_output_bytes: 131072, timeout_ms: 300000,
    description: "Delegate one to eight independent tasks and wait for their original outcomes. Children share this session's authority and budget.",
    input_schema: { type: "object", additionalProperties: false, required: ["tasks"], properties: {
      tasks: { type: "array", minItems: 1, maxItems: 8, items: { type: "object", additionalProperties: false,
        required: ["message"], properties: { message: { type: "string", minLength: 1 }, instructions: { type: "string" } } },
      },
    } },
  }];
}

function delegate(request, call, checkpoint) {
  const tasks = call.arguments?.tasks;
  if (!Array.isArray(tasks) || tasks.length < 1 || tasks.length > 8 ||
      tasks.some(task => typeof task?.message !== "string" || !task.message || Object.keys(task).some(key => !["message", "instructions"].includes(key)) ||
        (task.instructions !== undefined && typeof task.instructions !== "string"))) throw new Error("invalid delegated tasks");
  const delegations = tasks.map((task, index) => ({ request_id: "child-" + index, message: task.message, context: [],
    config: { ...structuredClone(request.config), ...(task.instructions === undefined ? {} : { instructions: task.instructions }) },
  }));
  checkpoint.awaiting = "children";
  checkpoint.call_id = call.id;
  checkpoint.children = delegations.length;
  return delegations.length === 1 ? { kind: "delegate", delegation: delegations[0], checkpoint }
    : { kind: "fork", delegations, checkpoint };
}

function childObservation(request, checkpoint) {
  if (checkpoint.awaiting !== "children" || request.children.length !== checkpoint.children ||
      request.children.some(child => child.availability === "pending")) throw new Error("delegated outcomes do not match the saved decision");
  const children = request.children.map(({ result, ...child }) => ({ ...child,
    ...(child.availability === "inline" ? { availability: "referenced" } : {}),
  }));
  const message = { role: "tool", tool_call_id: checkpoint.call_id, content: "" };
  const content = () => JSON.stringify({ kind: "untrusted_delegated_observations", children });
  for (let index = 0; index < children.length; index++) {
    if (request.children[index].availability !== "inline") continue;
    const prior = children[index];
    children[index] = request.children[index];
    message.content = content();
    if (Buffer.byteLength(JSON.stringify([...checkpoint.messages, message])) > MAX_CONTEXT_BYTES) children[index] = prior;
  }
  message.content = content();
  checkpoint.messages.push(message);
  delete checkpoint.awaiting;
  delete checkpoint.children;
}

function initialMessages(request) {
  const messages = [];
  if (request.config.instructions) messages.push({ role: "system", content: request.config.instructions });
  for (const record of request.context) {
    if (record.run_id === request.run_id || record.data.command_id === request.command_id) continue;
    if (record.kind === "input.accepted" && record.data.input?.kind === "message") {
      messages.push({ role: "user", content: record.data.input.text });
    } else if (record.kind === "run.completed" && record.data.result?.role === "assistant") {
      messages.push(record.data.result);
    } else if (record.kind === "input.accepted" && record.data.input?.kind === "event") {
      const event = record.data.input;
      const route = request.config.events?.find(route => route.name === event.name && route.version === event.version);
      if (route?.disposition === "context") {
        messages.push({ role: "user", content: `Untrusted application observation (${event.name}):\n${JSON.stringify(event.data)}` });
      }
    }
  }
  const text = request.input.kind === "message" ? request.input.text
    : `Untrusted application event:\n${JSON.stringify(request.input)}`;
  messages.push({ role: "user", content: text });
  while (Buffer.byteLength(JSON.stringify(messages)) > MAX_CONTEXT_BYTES && messages.length > 2) {
    messages.splice(messages[0].role === "system" ? 1 : 0, 1);
  }
  if (Buffer.byteLength(JSON.stringify(messages)) > MAX_CONTEXT_BYTES) throw new Error("input exceeds portable context bound");
  return messages;
}

/** A pure decision function. Models and actions are always requested from Brain. */
export function decide(request) {
  if (request.protocol !== "brain-decision/1") throw new Error("unsupported Brain decision protocol");
  const checkpoint = structuredClone(request.checkpoint ?? {});
  if (checkpoint.version !== undefined && checkpoint.version !== 1) throw new Error("unsupported checkpoint version");
  checkpoint.version = 1;
  checkpoint.messages ??= initialMessages(request);
  checkpoint.pending ??= [];
  if (request.children?.length) childObservation(request, checkpoint);
  if (request.outcome) {
    if (request.outcome.status !== "completed" || !request.outcome.result_available) throw new Error("operation did not complete with available bytes");
    if (checkpoint.awaiting === "model") {
      const response = request.outcome.result;
      if (!response?.message || response.message.role !== "assistant") throw new Error("invalid mediated model response");
      checkpoint.messages.push(response.message);
      checkpoint.pending = [...(response.message.tool_calls ?? [])];
      if (checkpoint.pending.length === 0) {
        return { kind: "complete", result: response.message, checkpoint: { version: 1 } };
      }
    } else if (checkpoint.awaiting === "action") {
      checkpoint.messages.push({ role: "tool", tool_call_id: checkpoint.call_id, content: JSON.stringify(request.outcome.result) });
    }
    delete checkpoint.awaiting;
  }
  if (checkpoint.pending.length > 0) {
    const call = checkpoint.pending.shift();
    if (call.name === DELEGATE && delegationEnabled(request)) return delegate(request, call, checkpoint);
    if (!request.config.actions.some(action => action.name === call.name)) throw new Error("model requested an unadmitted action");
    checkpoint.awaiting = "action";
    checkpoint.call_id = call.id;
    return { kind: "call", effect: "action", action: call.name, arguments: call.arguments, checkpoint };
  }
  if (Buffer.byteLength(JSON.stringify(checkpoint.messages)) > MAX_CONTEXT_BYTES) throw new Error("portable context limit reached; explicit new-run context selection is required");
  checkpoint.awaiting = "model";
  return { kind: "call", effect: "model", action: "generate",
    arguments: { messages: checkpoint.messages, actions: modelActions(request) }, checkpoint };
}
