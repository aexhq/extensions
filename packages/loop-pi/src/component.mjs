import { defineAgentloop } from "@aexhq/brain/agentloop";
import { runPi } from "./logic.mjs";

export const turn = defineAgentloop(async ctx => (await runPi(ctx)).result);
