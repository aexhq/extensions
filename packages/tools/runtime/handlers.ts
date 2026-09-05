import { execFile } from "node:child_process";
import { mkdir, readFile, readdir, rename, writeFile } from "node:fs/promises";
import { dirname, isAbsolute, relative, resolve } from "node:path";
import { promisify } from "node:util";

interface RuntimeContext {
  readonly signal: AbortSignal;
  readonly workspace: string;
}

function workspacePath(workspace: string, requested: string): string {
  const root = resolve(workspace);
  const target = resolve(root, requested);
  const within = relative(root, target);
  if (within === ".." || within.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`) || isAbsolute(within)) {
    throw new Error(`${requested} is outside the Environment workspace`);
  }
  return target;
}

export async function bash(input: { command: string }, context: RuntimeContext) {
  try {
    const { stdout, stderr } = await promisify(execFile)("bash", ["-lc", input.command], {
      signal: context.signal,
      cwd: context.workspace,
      maxBuffer: 16 * 1024 * 1024,
    });
    return { exit_code: 0, stdout, stderr };
  } catch (error) {
    const failure = error as { code?: unknown; stdout?: string; stderr?: string; message?: string };
    if (typeof failure.code === "number") {
      return { exit_code: failure.code, stdout: failure.stdout ?? "", stderr: failure.stderr ?? "" };
    }
    throw new Error(failure.stderr?.trim() || failure.message || "bash failed");
  }
}

export async function edit(input: { path: string; old_text: string; new_text: string }, context: RuntimeContext) {
  const target = workspacePath(context.workspace, input.path);
  const content = await readFile(target, "utf8");
  const first = content.indexOf(input.old_text);
  if (first < 0) throw new Error("old_text was not found");
  if (content.indexOf(input.old_text, first + input.old_text.length) >= 0) {
    throw new Error("old_text occurs more than once; provide a more specific match");
  }
  await writeFile(target, `${content.slice(0, first)}${input.new_text}${content.slice(first + input.old_text.length)}`);
  return { path: input.path, replacements: 1 };
}

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

export async function glob(input: { pattern: string; limit: number }, context: RuntimeContext) {
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

export async function grep(input: { pattern: string; path: string; limit: number }, context: RuntimeContext) {
  let stdout: string;
  const target = relative(resolve(context.workspace), workspacePath(context.workspace, input.path)) || ".";
  try {
    ({ stdout } = await promisify(execFile)(
      "rg",
      ["--line-number", "--no-heading", "--color", "never", "--regexp", input.pattern, "--", target],
      { signal: context.signal, cwd: context.workspace, maxBuffer: 16 * 1024 * 1024 },
    ));
  } catch (error) {
    const failure = error as { code?: unknown; stderr?: string; message?: string };
    if (failure.code === 1) return { matches: [], truncated: false };
    throw new Error(failure.stderr?.trim() || failure.message || "ripgrep failed");
  }
  const lines = stdout.split(/\r?\n/u).filter(Boolean);
  return { matches: lines.slice(0, input.limit), truncated: lines.length > input.limit };
}

export async function ls(input: { path: string; limit: number }, context: RuntimeContext) {
  const values = (await readdir(workspacePath(context.workspace, input.path), { withFileTypes: true }))
    .map((entry) => ({ name: entry.name, kind: entry.isDirectory() ? "dir" : "file" }));
  values.sort((left, right) => left.name.localeCompare(right.name));
  return { entries: values.slice(0, input.limit), truncated: values.length > input.limit };
}

export async function read(input: { path: string; offset: number; limit: number }, context: RuntimeContext) {
  const file = await readFile(workspacePath(context.workspace, input.path));
  const data = file.subarray(input.offset, input.offset + input.limit);
  if (data.includes(0)) throw new Error(`${input.path} is binary`);
  return {
    content: new TextDecoder().decode(data),
    bytes: data.byteLength,
    truncated: file.byteLength > input.offset + input.limit,
  };
}

export async function todo(input: { action: "get" } | { action: "set"; items: { text: string; done: boolean }[] }, context: RuntimeContext) {
  const directory = workspacePath(context.workspace, ".aex");
  const target = workspacePath(context.workspace, ".aex/todo.json");
  if (input.action === "set") {
    await mkdir(directory, { recursive: true });
    const temporary = workspacePath(context.workspace, `.aex/todo-${process.pid}.json`);
    await writeFile(temporary, JSON.stringify(input.items));
    await rename(temporary, target);
    return { items: input.items };
  }
  try {
    return { items: JSON.parse(await readFile(target, "utf8")) as { text: string; done: boolean }[] };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return { items: [] };
    throw error;
  }
}

export async function write(input: { path: string; content: string }, context: RuntimeContext) {
  const target = workspacePath(context.workspace, input.path);
  await mkdir(dirname(target), { recursive: true });
  await writeFile(target, input.content);
  return { path: input.path, bytes: new TextEncoder().encode(input.content).byteLength };
}
