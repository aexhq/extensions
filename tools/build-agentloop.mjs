import { build } from "esbuild";
import { transformAsync } from "@babel/core";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { agentloopRuntime } from "./build-agentloop-runtime.mjs";

const [componentEntry, factoryEntry, outputDirectory] = process.argv.slice(2);
if (componentEntry === undefined || factoryEntry === undefined || outputDirectory === undefined) {
  throw new Error("usage: build-agentloop <component-entry> <factory-entry> <output-directory>");
}

const output = path.resolve(outputDirectory);
await rm(output, { recursive: true, force: true });
await mkdir(output, { recursive: true });
const bundled = await build({
  entryPoints: [path.resolve(componentEntry)],
  bundle: true,
  format: "iife",
  globalName: "brainProgram",
  platform: "neutral",
  mainFields: ["module", "main"],
  external: ["brain:agentloop/host@0.2.0"],
  write: false,
  legalComments: "none",
});
const source = bundled.outputFiles[0];
if (source === undefined) throw new Error("esbuild produced no Agentloop source");
// The embedded JS engine has no ICU property tables; compile those regexes into ordinary ranges.
const compatible = await transformAsync(source.text, { babelrc: false, configFile: false,
  plugins: ["@babel/plugin-transform-unicode-property-regex"] });
await writeFile(path.join(output, "loop.program.js"), compatible.code);
await writeFile(path.join(output, "runtime.component.wasm"), await agentloopRuntime());
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
const bytes = await readFile(path.join(output, "runtime.component.wasm"));
if (!bytes.subarray(0, 4).equals(Buffer.from([0, 97, 115, 109]))) {
  throw new Error("componentize-js produced no WebAssembly binary");
}
