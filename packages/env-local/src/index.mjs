import { environment } from "@aexhq/brain";
import { z } from "zod";

export const local = environment({
  methods: { inspect: { effect: "none", description: "Inspect the Docker workspace and active invocations.", input_schema: z.toJSONSchema(z.strictObject({})) } },
  options: z.strictObject({ url: z.url(), token: z.string().min(1), profile: z.string().min(1) }),
  url: ({ url }) => url,
  credential: ({ token }) => token,
  configure: ({ profile }) => ({ profile }),
});
