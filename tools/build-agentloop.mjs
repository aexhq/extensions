import { build } from "esbuild";
import { fileURLToPath } from "node:url";

const name = process.argv[2];
if (!["pi", "codex", "opencode"].includes(name)) throw new Error("unknown agentloop package");
await build({
  entryPoints: [fileURLToPath(new URL(`../packages/loop-${name}/src/runtime.ts`, import.meta.url))],
  outfile: fileURLToPath(new URL(`../packages/loop-${name}/dist/runtime.mjs`, import.meta.url)),
  bundle: true, packages: "external", platform: "node", format: "esm", target: "node22", sourcemap: true,
});
if (name === "opencode") await build({
  entryPoints: [fileURLToPath(new URL("../packages/loop-opencode/src/harness-opencode-provider.ts", import.meta.url))],
  outfile: fileURLToPath(new URL("../packages/loop-opencode/dist/harness-opencode-provider.js", import.meta.url)),
  platform: "node", format: "esm", target: "node22",
});
