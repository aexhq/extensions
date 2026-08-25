import { componentize } from "@bytecodealliance/componentize-js";
import { compileTools } from "@aexhq/brain";
import { mkdir, readFile, writeFile } from "node:fs/promises";

const here = new URL("./", import.meta.url);
const names = ["bash", "edit", "glob", "grep", "ls", "read", "todo", "write"];

for (const name of names) {
  const selected = (await import(new URL(`./dist/${name}.js`, here))).default;
  const compiled = await compileTools([selected]);
  const definition = compiled.items[0]?.definition;
  const bundle = compiled.bundles[0];
  if (definition === undefined || bundle === undefined) {
    throw new Error(`${name} did not compile to one Environment-executed Tool bundle`);
  }
  const config = {
    definition,
    descriptor: {
      runtime: "node22",
      tool_name: definition.name,
      contract_digest: definition.contract_digest,
      bundle_digest: bundle.checksum,
    },
    bundleBase64: bundle.content_base64,
  };
  await writeFile(new URL(`./dist/${name}.component.json`, here), `${JSON.stringify(config)}\n`);
}

const source = await readFile(new URL("./dispatcher.mjs", here), "utf8");
const wit = await readFile(new URL(import.meta.resolve("@aexhq/brain/contracts/tool")), "utf8");
const output = await componentize(source, wit, {
  worldName: "tool",
  disableFeatures: ["http", "fetch-event"],
});
await mkdir(new URL("./dist", here), { recursive: true });
await writeFile(new URL("./dist/tool.component.wasm", here), output.component);

const selected = (await import(new URL("./dist/subagents.js", here))).default;
const compiled = await compileTools([selected]);
const definition = compiled.items[0]?.definition;
if (definition === undefined) throw new Error("subagents did not compile to one Tool definition");
await writeFile(
  new URL("./dist/subagents.component.json", here),
  `${JSON.stringify({ definition })}\n`,
);
const childSource = await readFile(new URL("./children-dispatcher.mjs", here), "utf8");
const childOutput = await componentize(childSource, wit, {
  worldName: "tool",
  disableFeatures: ["http", "fetch-event"],
});
await writeFile(new URL("./dist/children.component.wasm", here), childOutput.component);
