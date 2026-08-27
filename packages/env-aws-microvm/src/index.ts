import { environment } from "@aexhq/brain";
import { z } from "zod";

const options = z.object({
  region: z.string().min(1).optional(),
  idleSeconds: z.number().int().positive().optional(),
  maximumSeconds: z.number().int().positive().optional(),
}).strict().default({}).refine((value) => value.idleSeconds === undefined || value.maximumSeconds === undefined || value.idleSeconds <= value.maximumSeconds, {
  message: "idleSeconds cannot exceed maximumSeconds",
}).transform((value) => ({
  ...(value.region === undefined ? {} : { region: value.region }),
  ...(value.idleSeconds === undefined ? {} : { idle_seconds: value.idleSeconds }),
  ...(value.maximumSeconds === undefined ? {} : { maximum_seconds: value.maximumSeconds }),
}));

export const awsMicroVm = environment({ options }, (author) => {
  const vm = author.open(async () => ({}));
  vm.run(async () => { throw new Error("AWS MicroVM Tools execute in the Rust provider runtime"); });
  vm.close(async () => undefined);
  return {
    suspend: vm.method(async () => undefined),
  };
});
