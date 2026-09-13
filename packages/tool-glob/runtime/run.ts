import { readdir } from "node:fs/promises";
import type { z } from "zod";
import type { inputSchema } from "../src/schema.js";
import { workspacePath, type RuntimeContext } from "../../../shared/tool-runtime.js";

function globPattern(pattern: string): RegExp {
  const normalized = pattern.replaceAll("\\", "/").replace(/^\.\//u, "");
  let source = "^";
  for (let index = 0; index < normalized.length; index += 1) {
    const character = normalized[index] as string;
    if (character === "*") {
      if (normalized[index + 1] === "*") {
        index += 1;
        source += "(?:[^/]+/)*";
        if (normalized[index + 1] === "/") index += 1;
        else source += "[^/]*";
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

export async function run(input: z.output<typeof inputSchema>, context: RuntimeContext) {
  const matches = globPattern(input.pattern);
  const maximumDepth = input.pattern.includes("**") ? Number.POSITIVE_INFINITY : input.pattern.replaceAll("\\", "/").split("/").length;
  const paths: string[] = [];
  let truncated = false;
  let visited = 0;
  const walk = async (directory: string, depth: number): Promise<void> => {
    if (truncated || depth > maximumDepth || (visited += 1) > 10_000) return;
    const entries = await readdir(workspacePath(context.workspace, directory), { withFileTypes: true });
    entries.sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      if (truncated) return;
      const path = directory === "." ? entry.name : `${directory}/${entry.name}`;
      if (matches.test(path)) {
        if (paths.length >= input.limit) {
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
}
