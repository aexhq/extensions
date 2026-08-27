import { buildAgentloop } from "@aexhq/agentloop/build";
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const here = (path) => fileURLToPath(new URL(path, import.meta.url));
await mkdir(here("./dist"), { recursive: true });
const built = await buildAgentloop({ entry: here("./src/loop.mjs"), out: here("./dist/loop.brain.json") });
console.error(`pi -> ${built.manifest.component_digest} (${built.manifest.component_bytes} bytes)`);
