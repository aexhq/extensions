import { environment, inspectTool, bindTool } from "@aexhq/brain";
import { z } from "zod";

export const http = environment({
  options: z.strictObject({ url: z.url(), token: z.string().min(1).optional(), binding: z.string().min(1) }),
  url: ({ url }) => url,
  credential: ({ token }) => token,
  configure: ({ binding }) => ({ binding }),
});

export function httpTool(tool, { env }) {
  const { definition } = inspectTool(tool);
  return bindTool({ definition }, { type: "http_tool", definition })({ env });
}
