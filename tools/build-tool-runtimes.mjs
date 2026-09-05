import { createHash } from "node:crypto";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { environment, inspectPlacedTool } from "@aexhq/brain";
import { build } from "esbuild";

const output = path.resolve("dist/runtime");
await mkdir(output, { recursive: true });
const definitions = await import(`${pathToFileURL(path.resolve("dist/index.js")).href}?build=${Date.now()}`);
const env = environment({ driver: "tool-build" })();
const registry = {};

for (const name of Object.keys(definitions).sort()) {
  const factory = definitions[name];
  if (typeof factory !== "function") continue;
  const source = inspectPlacedTool(factory({ env }));
  const manifest = {
    name: source.definition.name,
    description: source.definition.description,
    input_schema: source.definition.inputSchema,
    ...(source.definition.outputSchema === undefined ? {} : { output_schema: source.definition.outputSchema }),
    needs: [...source.needs],
    binding_names: [...source.bindingNames],
    implementation: source.implementation,
  };
  const contractDigest = createHash("sha256").update(canonicalJson(manifest)).digest("hex");
  const filename = `${name}.mjs`;
  const metadata = JSON.stringify({ kind: "brain.tool-runtime", name, contractDigest, requiredEnv: [] });
  await build({
    stdin: {
      contents: `import { ${name} as run } from "./runtime/handlers.ts";\nimport { ${name}Input as input, ${name}Output as output } from "./src/schemas.ts";\nasync function execute(value, context) { return output.parse(await run(input.parse(value), context)); }\nexport default { ...${metadata}, execute };\n`,
      resolveDir: process.cwd(),
      sourcefile: `${name}.runtime.ts`,
      loader: "ts",
    },
    bundle: true,
    format: "esm",
    platform: "node",
    target: "node22",
    outfile: path.join(output, filename),
    legalComments: "none",
  });
  registry[name] = { contract_digest: contractDigest, filename, manifest };
}

await writeFile(path.join(output, "registry.json"), `${JSON.stringify(registry, null, 2)}\n`);

function canonicalJson(value) {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
}
