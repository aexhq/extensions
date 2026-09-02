import { readdir } from "node:fs/promises";

import { tool } from "@aexhq/brain";
import { z } from "zod";

const globInput = z.object({ pattern: z.string().min(1), limit: z.number().int().positive().max(10_000).default(1_000) });
const globOutput = z.object({ paths: z.array(z.string()), truncated: z.boolean() });

/** Compile a glob into a full-path regular expression: `**` crosses directory
 * separators, `*` and `?` stay within one segment. */
function globPattern(pattern: string): RegExp {
  const normalized = pattern.replaceAll("\\", "/").replace(/^\.\//u, "");
  let source = "^";
  for (let index = 0; index < normalized.length; index += 1) {
    const character = normalized[index] as string;
    if (character === "*") {
      if (normalized[index + 1] === "*") {
        index += 1;
        if (normalized[index + 1] === "/") index += 1;
        source += "(?:[^/]+/)*[^/]*";
      } else {
        source += "[^/]*";
      }
    } else if (character === "?") {
      source += "[^/]";
    } else {
      source += ".+^${}()|[]\\".includes(character) ? `\\${character}` : character;
    }
  }
  return new RegExp(`${source}$`, "u");
}

const MAX_VISITED_DIRECTORIES = 10_000;

export const glob = tool({
  description: "List Environment workspace paths matching a glob pattern.",
  input: globInput,
  output: globOutput,
  needs: ["fs"],
}, (author) => {
  author.run(async ({ pattern, limit }) => {
    const matches = globPattern(pattern);
    const maximumDepth = pattern.includes("**") ? Number.POSITIVE_INFINITY : pattern.replaceAll("\\", "/").split("/").length;
    const paths: string[] = [];
    let truncated = false;
    let visited = 0;
    const walk = async (directory: string, depth: number): Promise<void> => {
      if (truncated || depth > maximumDepth || (visited += 1) > MAX_VISITED_DIRECTORIES) return;
      const entries = await readdir(directory, { withFileTypes: true });
      entries.sort((left, right) => left.name.localeCompare(right.name));
      for (const entry of entries) {
        if (truncated) return;
        const path = directory === "." ? entry.name : `${directory}/${entry.name}`;
        if (matches.test(path)) {
          if (paths.length >= limit) {
            truncated = true;
            return;
          }
          paths.push(path);
        }
        if (entry.isDirectory()) await walk(path, depth + 1);
      }
    };
    await walk(".", 1);
    paths.sort();
    return { paths, truncated };
  });
});
