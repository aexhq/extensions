import { componentize } from "@bytecodealliance/componentize-js";
import { build } from "esbuild";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const here = new URL("./", import.meta.url);
const bundled = await build({ entryPoints: [fileURLToPath(new URL("./adapter.mjs", here))], bundle: true, format: "esm", platform: "neutral", external: ["aex:model/host@1.0.0"], write: false, legalComments: "none" });
const source = bundled.outputFiles[0]?.text;
if (source === undefined) throw new Error("esbuild produced no Model bundle");
const wit = await readFile(new URL(import.meta.resolve("@aexhq/brain/contracts/model")), "utf8");
const output = await componentize(source, wit, { worldName: "model", disableFeatures: ["http", "fetch-event"] });
await mkdir(new URL("./dist", here), { recursive: true });
await writeFile(new URL("./dist/model.component.wasm", here), output.component);
