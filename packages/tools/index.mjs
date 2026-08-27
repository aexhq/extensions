import bashTool from "./dist/bash.js";
import editTool from "./dist/edit.js";
import globTool from "./dist/glob.js";
import grepTool from "./dist/grep.js";
import lsTool from "./dist/ls.js";
import readTool from "./dist/read.js";
import todoTool from "./dist/todo.js";
import writeTool from "./dist/write.js";

const tools = { bash: bashTool, edit: editTool, glob: globTool, grep: grepTool, ls: lsTool, read: readTool, todo: todoTool, write: writeTool };

export const definitions = Object.freeze(Object.fromEntries(Object.entries(tools).map(([name, tool]) => [name, Object.freeze({ definition: tool.definition, remoteToolId: name })])));
export const handlers = Object.freeze(Object.fromEntries(Object.entries(tools).map(([name, tool]) => [name, tool.execute.bind(tool)])));

export const bash = () => definitions.bash;
export const edit = () => definitions.edit;
export const glob = () => definitions.glob;
export const grep = () => definitions.grep;
export const ls = () => definitions.ls;
export const read = () => definitions.read;
export const todo = () => definitions.todo;
export const write = () => definitions.write;
