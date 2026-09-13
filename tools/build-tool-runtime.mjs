import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { environment, inspectTool } from "@aexhq/brain";
import { build } from "esbuild";

const output = path.resolve("dist/runtime");
await mkdir(output, { recursive: true });
const packageDocument = JSON.parse(await readFile("package.json", "utf8"));
const name = packageDocument.name.slice("@aexhq/tool-".length);
const { [name]: factory } = await import(pathToFileURL(path.resolve("dist/index.js")).href);
const env = environment({ url: () => "https://build.example" })({ name: "build" });
const source = inspectTool(factory({ env }));
const manifest = {
  name: source.definition.name,
  description: source.definition.description,
  input_schema: source.definition.inputSchema,
  output_schema: source.definition.outputSchema,
  implementation: source.implementation,
};
const contractDigest = createHash("sha256").update(canonicalJson(manifest)).digest("hex");
const filename = `${name}.mjs`;
const metadata = JSON.stringify({ kind: "brain.tool-runtime", name, contractDigest, requiredEnv: [] });
await build({
  stdin: {
    contents: `import { run } from "./runtime/run.ts";\nimport { inputSchema, outputSchema } from "./src/schema.ts";\nasync function execute(value, context) { return outputSchema.parse(await run(inputSchema.parse(value), context)); }\nexport default { ...${metadata}, execute };\n`,
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
await writeFile(path.join(output, "registry.json"), `${JSON.stringify({
  [name]: { contract_digest: contractDigest, filename, manifest },
}, null, 2)}\n`);

function canonicalJson(value) {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
}
