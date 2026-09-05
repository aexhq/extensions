import { componentize } from "@bytecodealliance/componentize-js";
import { build } from "esbuild";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const [componentEntry, factoryEntry, outputDirectory] = process.argv.slice(2);
if (componentEntry === undefined || factoryEntry === undefined || outputDirectory === undefined) {
  throw new Error("usage: build-agentloop <component-entry> <factory-entry> <output-directory>");
}

const output = path.resolve(outputDirectory);
const work = await mkdtemp(path.join(tmpdir(), "aex-agentloop-build-"));
try {
  await rm(output, { recursive: true, force: true });
  await mkdir(output, { recursive: true });
  const bundled = await build({
    entryPoints: [path.resolve(componentEntry)],
    bundle: true,
    format: "esm",
    platform: "neutral",
    external: ["brain:agentloop/host@0.1.0"],
    write: false,
    legalComments: "none",
  });
  const source = bundled.outputFiles[0];
  if (source === undefined) throw new Error("esbuild produced no Agentloop source");
  const sourcePath = path.join(work, "agentloop.mjs");
  await writeFile(sourcePath, source.contents);
  const witPath = fileURLToPath(new URL(import.meta.resolve("@aexhq/brain/contracts/agentloop.wit")));
  const built = await componentize({
    sourcePath,
    witPath,
    worldName: "agentloop",
    disableFeatures: ["stdio", "random", "clocks", "http", "fetch-event"],
  });
  await writeFile(path.join(output, "loop.component.wasm"), built.component);
  await build({
    entryPoints: [path.resolve(factoryEntry)],
    bundle: true,
    format: "esm",
    platform: "node",
    target: "node22",
    external: ["@aexhq/brain", "zod"],
    outfile: path.join(output, "index.mjs"),
    legalComments: "none",
  });
  const bytes = await readFile(path.join(output, "loop.component.wasm"));
  if (!bytes.subarray(0, 4).equals(Buffer.from([0, 97, 115, 109]))) {
    throw new Error("componentize-js produced no WebAssembly binary");
  }
} finally {
  await rm(work, { recursive: true, force: true });
}
