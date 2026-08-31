import { tool } from "@aexhq/brain";
import { z } from "zod";

const editInput = z.object({ path: z.string().min(1), old_text: z.string().min(1), new_text: z.string() });
const editOutput = z.object({ path: z.string(), replacements: z.literal(1) });

export const edit = tool({
  description: "Replace one exact occurrence of text in an Environment workspace file.",
  input: editInput,
  output: editOutput,
  requires: ["fs"],
}, (author) => {
  author.run(async ({ path, old_text, new_text }, context) => {
    const content = new TextDecoder().decode(await context.fs.read(path));
    const first = content.indexOf(old_text);
    if (first < 0) throw new Error("old_text was not found");
    if (content.indexOf(old_text, first + old_text.length) >= 0) {
      throw new Error("old_text occurs more than once; provide a more specific match");
    }
    await context.fs.write(path, `${content.slice(0, first)}${new_text}${content.slice(first + old_text.length)}`);
    return { path, replacements: 1 as const };
  });
});
