import { environment, inspectTool, bindTool } from "@aexhq/brain";
import { z } from "zod";

export const http = environment({
  methods: { inspect: { effect: "none", description: "Inspect the authorized HTTP binding and Tools without contacting the application.", input_schema: z.toJSONSchema(z.strictObject({})) } },
  options: z.strictObject({ url: z.url(), token: z.string().min(1).optional(), binding: z.string().min(1) }),
  url: ({ url }) => url,
  credential: ({ token }) => token,
  configure: ({ binding }) => ({ binding }),
});

export const application = environment({
  options: z.strictObject({ url: z.url(), endpoint: z.url(), credential: z.string().min(1),
    timeoutMs: z.number().int().positive().max(300_000).default(30_000) }),
  url: ({ url }) => url,
  credential: ({ credential }) => credential,
  configure: ({ endpoint, timeoutMs }) => ({ type: "application", endpoint, timeoutMs }),
});

export function applicationTool(tool, { env }) {
  const source = inspectTool(tool);
  return bindTool({ definition: source.definition }, {
    type: "application_tool", definition: source.definition, options: source.configuration,
  })({ env, ...(source.environments === undefined ? {} : { environments: source.environments }) });
}

export function httpTool(tool, { env }) {
  const { definition } = inspectTool(tool);
  return bindTool({ definition }, { type: "http_tool", definition })({ env });
}
