#!/usr/bin/env node
process.env.PI_OFFLINE = "1";
console.log = console.error;
const { serveHarness } = await import("@aexhq/brain/harness");
const { piHarness } = await import("../dist/runtime.mjs");
await serveHarness(piHarness());
