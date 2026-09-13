import { glob } from "node:fs/promises";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

export async function packageWorkspaces() {
  const cwd = fileURLToPath(new URL("../packages/", import.meta.url));
  const manifests = await Array.fromAsync(glob("*/package.json", { cwd }));
  return manifests.map(filename => dirname(filename)).sort();
}
