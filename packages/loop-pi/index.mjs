import { defineAgentLoop } from "@aexhq/brain";

export const packageUrl = new URL("./dist/loop.brain.json", import.meta.url);
export const pi = () => defineAgentLoop(packageUrl);
