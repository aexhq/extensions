import { componentize } from "@bytecodealliance/componentize-js";
import { build } from "esbuild";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const here = new URL("./", import.meta.url);

async function bundle(entry, host) {
  const built = await build({
    entryPoints: [fileURLToPath(new URL(entry, here))],
    bundle: true,
    format: "esm",
    platform: "neutral",
    external: [host],
    write: false,
    legalComments: "none",
  });
  const source = built.outputFiles[0]?.text;
  if (source === undefined) throw new Error(`esbuild produced no bundle for ${entry}`);
  return source;
}

await mkdir(new URL("./dist", here), { recursive: true });

const environmentWit = await readFile(
  new URL(import.meta.resolve("@aexhq/brain/contracts/environment")),
  "utf8",
);
const environment = await componentize(
  await bundle("./adapter.mjs", "aex:environment/host@1.0.0"),
  environmentWit,
  { worldName: "environment", disableFeatures: ["http", "fetch-event"] },
);
await writeFile(new URL("./dist/environment.component.wasm", here), environment.component);

const toolWit = await readFile(new URL(import.meta.resolve("@aexhq/brain/contracts/tool")), "utf8");
const tool = await componentize(
  await bundle("./tool.mjs", "aex:tool/environment@1.0.0"),
  toolWit,
  { worldName: "tool", disableFeatures: ["http", "fetch-event"] },
);
await writeFile(new URL("./dist/tool.component.wasm", here), tool.component);
