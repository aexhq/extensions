import { componentize } from "@bytecodealliance/componentize-js";
import { mkdir, readFile, writeFile } from "node:fs/promises";

const here = new URL("./", import.meta.url);
const source = await readFile(new URL("./adapter.mjs", here), "utf8");
const wit = await readFile(
  new URL(import.meta.resolve("@aexhq/brain/contracts/environment")),
  "utf8",
);
const output = await componentize(source, wit, {
  worldName: "environment",
  disableFeatures: ["http", "fetch-event"],
});
await mkdir(new URL("./dist", here), { recursive: true });
await writeFile(new URL("./dist/environment.component.wasm", here), output.component);
