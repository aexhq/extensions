import { isAbsolute, relative, resolve } from "node:path";

export interface RuntimeContext {
  readonly signal: AbortSignal;
  readonly workspace: string;
}

export function workspacePath(workspace: string, requested: string): string {
  const root = resolve(workspace);
  const target = resolve(root, requested);
  const within = relative(root, target);
  if (within === ".." || within.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`) || isAbsolute(within)) {
    throw new Error(`${requested} is outside the Environment workspace`);
  }
  return target;
}
