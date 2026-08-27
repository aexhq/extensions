#!/usr/bin/env node
import { buildAgentloop } from "./build.js";

const [, , command, entry, flag, out] = process.argv;
if (command !== "build" || entry === undefined || flag !== "--out" || out === undefined) {
  console.error("usage: brain-loop build <entry> --out <package.json>");
  process.exit(2);
}
await buildAgentloop({ entry, out });
