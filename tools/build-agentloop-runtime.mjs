import { componentize } from "@bytecodealliance/componentize-js";
import { createHash, randomUUID } from "node:crypto";
import { link, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

export async function agentloopRuntime() {
  const sourcePath = fileURLToPath(new URL("./agentloop-runtime.mjs", import.meta.url));
  const witPath = fileURLToPath(new URL(import.meta.resolve("@aexhq/brain/contracts/agentloop.wit")));
  const digest = createHash("sha256");
  for (const file of [sourcePath, witPath, import.meta.filename, new URL("../package-lock.json", import.meta.url)]) {
    digest.update(await readFile(file));
  }
  const directory = path.join(tmpdir(), "aex-agentloop-runtime");
  await mkdir(directory, { recursive: true });
  const target = path.join(directory, `${digest.digest("hex")}.wasm`);
  try { return await readFile(target); }
  catch (error) { if (error.code !== "ENOENT") throw error; }
  // Component snapshots vary between builds; every package must copy the same artifact.
  const built = await componentize({ sourcePath, witPath, worldName: "agentloop",
    disableFeatures: ["stdio", "random", "clocks", "http", "fetch-event"] });
  const temporary = path.join(directory, `${randomUUID()}.wasm`);
  try {
    await writeFile(temporary, built.component);
    try { await link(temporary, target); }
    catch (error) { if (error.code !== "EEXIST") throw error; }
    return await readFile(target);
  } finally { await rm(temporary, { force: true }); }
}
