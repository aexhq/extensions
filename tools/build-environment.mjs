import { build } from "esbuild";
import { readdir } from "node:fs/promises";

const entryPoints = (await readdir("src")).filter(name => ["index.mjs", "server.mjs"].includes(name)).map(name => `src/${name}`);
await build({ entryPoints, outdir: "dist", outExtension: { ".js": ".mjs" }, bundle: true,
  packages: "external", platform: "node", format: "esm", target: "node22", legalComments: "none" });
