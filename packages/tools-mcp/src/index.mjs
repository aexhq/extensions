import { Client, ProtocolError, SdkError, SdkErrorCode } from "@modelcontextprotocol/client";
import { AjvJsonSchemaValidator } from "@modelcontextprotocol/client/validators/ajv";
import { inspectEnvironment, tool } from "@aexhq/brain";
import { z } from "zod";
import { toolOutput } from "../../../shared/tool-output.mjs";

export async function connectMcp(transport, { name = "aex-mcp", version = "0.1.0", ...options } = {}) {
  const client = new Client({ name, version }, { ...options, inputRequired: { autoFulfill: false } });
  try { await client.connect(transport); }
  catch (error) { await client.close(); throw error; }
  return client;
}

export async function mcpTools({ client, names, env, prefix = "", project }) {
  if (inspectEnvironment(env).driver.driver !== "host") throw new TypeError("MCP Tools must be placed in hostEnv");
  z.array(z.string().min(1)).min(1).parse(names);
  if (new Set(names).size !== names.length) throw new TypeError("select each MCP Tool once");
  const discovered = await client.listTools(undefined, { cacheMode: "refresh" });
  const selected = names.map(name => {
    const matches = discovered.tools.filter(definition => definition.name === name);
    if (matches.length !== 1) throw new Error(`expected one MCP definition for ${name}, received ${matches.length}`);
    const definition = structuredClone(matches[0]);
    if (definition.execution?.taskSupport === "required") throw new Error(`${name} requires unsupported MCP task execution`);
    return definition;
  });
  return selected.map(definition => {
    const validate = new AjvJsonSchemaValidator().getValidator(definition.inputSchema);
    const input = z.unknown().superRefine((value, context) => {
      const result = validate(value);
      if (!result.valid) context.addIssue({ code: "custom", message: result.errorMessage });
    }).meta(definition.inputSchema);
    return tool({ name: `${prefix}${definition.name}`, description: definition.description || definition.name, input,
      run: async (arguments_, context) => {
        context.signal.throwIfAborted();
        let response;
        try {
          response = await client.callTool({ name: definition.name, arguments: arguments_ }, {
            signal: context.signal, timeout: Math.max(1, context.deadline.getTime() - Date.now()),
            toolDefinition: definition, allowInputRequired: true,
          });
        } catch (error) {
          if (context.signal.aborted) throw error;
          const message = String(error.message ?? error);
          const details = { code: error.code ?? null, data: error.data ?? null };
          const outcome = error instanceof SdkError && error.code === SdkErrorCode.RequestTimeout
            ? { status: "timeout" }
            : error instanceof ProtocolError
              ? failure("mcp_protocol_error", message, details)
              : error instanceof SdkError && ![SdkErrorCode.ConnectionClosed, SdkErrorCode.SendFailed].includes(error.code)
                ? failure("mcp_sdk_error", message, details)
                : { status: "unknown", message: `MCP ${definition.name}: outcome unknown; ${message}` };
          await context.emit("mcp_failure", { tool: definition.name, outcome: outcome.status,
            code: details.code, message, details: details.data });
          return outcome;
        }
        const evidenceSequence = await context.emit("mcp_result", { tool: definition.name, result: response });
        if (response.resultType && response.resultType !== "complete") {
          return failure("mcp_unsupported_result", `MCP ${definition.name} requires unsupported continuation (${response.resultType}); evidence ${evidenceSequence}`, response);
        }
        if (response.isError) {
          const message = response.content?.filter(block => block.type === "text").map(block => block.text).join("\n");
          return failure("mcp_tool_error", `MCP ${definition.name}: ${message || "Tool failed"}; evidence ${evidenceSequence}`, response);
        }
        const media = [];
        const content = [];
        for (const block of response.content ?? []) {
          if (block.type === "image") {
            if (!["image/png", "image/jpeg", "image/gif", "image/webp"].includes(block.mimeType)) throw new Error(`unsupported MCP image type: ${block.mimeType}`);
            media.push({ type: "image", url: `data:${block.mimeType};base64,${block.data}` });
          } else if (block.type === "text" || block.type === "resource_link" || (block.type === "resource" && typeof block.resource?.text === "string")) {
            content.push(block);
          } else throw new Error(`unsupported MCP content type: ${block.type}; evidence ${evidenceSequence}`);
        }
        const presented = { content, ...(response.structuredContent === undefined ? {} : { structuredContent: response.structuredContent }), evidenceSequence };
        return toolOutput(project ? await project(presented, definition.name) : presented, media);
      },
    })({ env });
  });
}

const failure = (code, message, details) => ({ status: "error", error: {
  code, message: message.slice(0, 4096), retryable: false, details,
} });
