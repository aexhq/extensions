import { environment } from "@aexhq/brain";

export const app = environment((author) => {
  const process = author.open(async () => ({}));
  process.run(async () => { throw new Error("Application Tools execute in the attached application provider"); });
  process.close(async () => undefined);
  return {};
});
