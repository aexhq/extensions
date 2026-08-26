import { component } from "@aexhq/brain";
import { readFile } from "node:fs/promises";

const asset = new URL("./dist/tool.component.wasm", import.meta.url);

async function load(name) {
  return JSON.parse(await readFile(new URL(`./dist/${name}.component.json`, import.meta.url), "utf8"));
}

const configs = Object.freeze({
  bash: await load("bash"),
  edit: await load("edit"),
  glob: await load("glob"),
  grep: await load("grep"),
  ls: await load("ls"),
  read: await load("read"),
  todo: await load("todo"),
  write: await load("write"),
});

const subagentsTool = (await import("./dist/subagents.js")).default;

function official(name) {
  return component("tool", asset, configs[name], {
    grants: ["environment"],
    metadata: { name, source: "@aexhq/tools" },
    bundle: new URL(`./dist/${name}.bundle.mjs`, import.meta.url),
  });
}

export const bash = () => official("bash");
export const edit = () => official("edit");
export const glob = () => official("glob");
export const grep = () => official("grep");
export const ls = () => official("ls");
export const read = () => official("read");
export const todo = () => official("todo");
export const write = () => official("write");
/** Brain's builtin child-session capability. Turning it on is declaring it. */
export const subagents = () => subagentsTool;
export const task = subagents;
