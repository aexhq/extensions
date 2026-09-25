import { z } from "zod";

const named = z.strictObject({ name: z.string().regex(/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u) });
export const methods = {
  inspect: { description: "Inspect the browser, named pages, selection and active invocations.", input: z.strictObject({}) },
  open_page: { description: "Create a named blank page and select it. Navigate with browser_navigate.", input: named },
  select_page: { description: "Select the named page for subsequent browser Tools.", input: named },
  close_page: { description: "Close a named page. Select another page before using Tools if it was selected.", input: named },
};
export const definitions = Object.fromEntries(Object.entries(methods).map(([name, { description, input }]) =>
  [name, { description, input_schema: z.toJSONSchema(input), effect: "none" }]));
