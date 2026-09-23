import { execFileSync } from "node:child_process";
import { mkdir, readdir, unlink } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { packageTools } from "@aexhq/brain/package";
import { packageWorkspaces } from "./workspaces.mjs";

const destination = resolve(".runtime-packages");
await mkdir(destination, { recursive: true });
for (const name of await readdir(destination)) await unlink(resolve(destination, name));
const fixture = resolve("test/fixtures/package-tool");
await mkdir(resolve(fixture, "dist"), { recursive: true });
await packageTools(fixture);
const directories = [dirname(fileURLToPath(import.meta.resolve("@aexhq/brain/package.json"))), fixture,
  ...(await packageWorkspaces()).filter(name => name.startsWith("tool-")).map(name => resolve("packages", name))];
for (const directory of directories) {
  execFileSync(process.execPath, [process.env.npm_execpath, "pack", "--ignore-scripts", "--pack-destination", destination], { cwd: directory, stdio: "pipe" });
}
