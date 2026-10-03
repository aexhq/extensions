import { defineAgentloop } from "@aexhq/brain/agentloop";
import { runCodex } from "./logic.mjs";

export const turn = defineAgentloop(async ctx => (await runCodex(ctx)).result);
