import { environment } from "@aexhq/brain";
import { z } from "zod";

const options = z.object({
  url: z.url(),
  token: z.string().min(1),
  region: z.string().min(1).optional(),
}).strict();

export const awsMicroVm = environment({
  options,
  url: ({ url }) => url,
  credential: ({ token }) => token,
  configure: (value) => ({
    driver: "aws-microvm",
    ...(value.region === undefined ? {} : { region: value.region }),
  }),
});
