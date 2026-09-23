import { readFile } from "node:fs/promises";
import { tool } from "@aexhq/brain";
import { z } from "zod";

export const report = tool({ name: "third_party_report", description: "Read and summarize a file.",
  input: z.object({ path: z.string() }),
  run: async ({ path }, context) => {
    const document = await readFile(path, "utf8");
    await context.emitResult({ document }, { content: "Read the file." });
    const summaries = await Promise.all(["one", "two"].map(text => context.model({ messages: [{ role: "user", content: [{ type: "text", text }] }] })));
    return context.finish({ document, summaries }, { content: "Summarized the file." });
  },
});
