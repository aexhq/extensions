// Build the publishable component with the same public authoring API available to outsiders.
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { LOOP_TOOLCHAIN, buildAgentloopComponent } from "@aexhq/agentloop/build";
import { componentize } from "@bytecodealliance/componentize-js";

const here = (path) => fileURLToPath(new URL(path, import.meta.url));
const pkg = JSON.parse(await readFile(here("./package.json"), "utf8"));

const bundle = await buildAgentloopComponent({ entry: here("./src/loop.mjs") }, componentize);

await mkdir(here("./dist"), { recursive: true });
await writeFile(here("./dist/loop.component.wasm"), bundle.component);
await writeFile(
  here("./dist/identity.json"),
  `${JSON.stringify(
    {
      name: pkg.agentloop.name,
      version: pkg.version,
      toolchain: LOOP_TOOLCHAIN,
      source_bundle_sha256: bundle.sha256,
      component_sha256: bundle.componentSha256,
      bytes: bundle.componentBytes,
    },
    null,
    2,
  )}\n`,
);
// stderr: `npm pack --json` consumers parse stdout.
console.error(
  `${pkg.agentloop.name}@${pkg.version} -> ${bundle.componentSha256} (${bundle.componentBytes} bytes)`,
);
