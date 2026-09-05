import { environment } from "@aexhq/brain";
import { z } from "zod";

const options = z.object({
  region: z.string().min(1).optional(),
  idleSeconds: z.number().int().positive().optional(),
  maximumSeconds: z.number().int().positive().optional(),
}).strict().default({}).refine(
  (value) => value.idleSeconds === undefined || value.maximumSeconds === undefined || value.idleSeconds <= value.maximumSeconds,
  { message: "idleSeconds cannot exceed maximumSeconds" },
);

export const awsMicroVm = environment({
  driver: "aws-microvm",
  options,
  configure: (value) => ({
    ...(value.region === undefined ? {} : { region: value.region }),
    ...(value.idleSeconds === undefined ? {} : { idle_seconds: value.idleSeconds }),
    ...(value.maximumSeconds === undefined ? {} : { maximum_seconds: value.maximumSeconds }),
  }),
});
