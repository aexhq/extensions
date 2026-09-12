import { environment, tool } from "@aexhq/brain";
import { z } from "zod";
import { definitions } from "./definitions.mjs";

export const browser = environment({
  options: z.strictObject({ url: z.url(), token: z.string().min(1), profile: z.string().min(1) }),
  url: ({ url }) => url, credential: ({ token }) => token, configure: ({ profile }) => ({ profile }),
});
const factories = Object.fromEntries(Object.entries(definitions).map(([name, definition]) => [name, tool({
  name, ...definition, implementation: { type: "aex_browser_tool", version: 1, name },
})]));
export const navigate = factories.browser_navigate;
export const inspect = factories.browser_inspect;
export const click = factories.browser_click;
export const fill = factories.browser_fill;
export const screenshot = factories.browser_screenshot;
export const browserTools = ({ env }) => Object.values(factories).map(factory => factory({ env }));
