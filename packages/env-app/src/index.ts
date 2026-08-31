import { environment } from "@aexhq/brain";
import { z } from "zod";

const options = z.object({ channelToken: z.string().min(1) }).strict();

export const app = environment({ options }, (author) => {
  const process = author.open(async () => ({}));
  process.run(async () => {
    throw new Error("application Tools are callback-hosted; the callback router answers their invocations");
  });
  process.close(async () => undefined);
  // Callback tools execute in the author's application process. The app holds an
  // outbound WebSocket to this Environment's channel, authenticated by the
  // configured channelToken; each invocation travels down it as one frame.
  author.route.callbacks();
  return {};
});
