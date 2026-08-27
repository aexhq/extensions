import { build } from "esbuild";
import { createHash } from "node:crypto";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

const names = ["bash", "edit", "glob", "grep", "ls", "read", "todo", "write"];
const packageRoot = import.meta.dirname;
const directory = path.join(packageRoot, "dist/runtime");
await mkdir(directory, { recursive: true });
const registry = {};

for (const name of names) {
  const imported = await import(`./dist/${name}.js`);
  const definition = imported.default.definition;
  const contractDigest = createHash("sha256").update(canonical(definition)).digest("hex");
  await build({
    stdin: {
      contents: `
        import tool from ${JSON.stringify(`./dist/${name}.js`)};
        export default {
          kind: "brain.tool-runtime",
          name: ${JSON.stringify(name)},
          description: ${JSON.stringify(definition.description)},
          contractDigest: ${JSON.stringify(contractDigest)},
          requiredEnv: [],
          execute(input, context) { return tool.execute(input, { ...context, grant: null }); }
        };
      `,
      resolveDir: packageRoot,
      sourcefile: `${name}-runtime.js`,
      loader: "js",
    },
    bundle: true,
    format: "esm",
    platform: "node",
    target: "node22",
    outfile: path.join(directory, `${name}.mjs`),
    legalComments: "none",
  });
  registry[name] = { contract_digest: contractDigest, filename: `${name}.mjs` };
}
await writeFile(path.join(directory, "registry.json"), `${JSON.stringify(registry, null, 2)}\n`);

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value !== null && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}
