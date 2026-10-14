#!/usr/bin/env bun
console.log = console.error;
const { serveHarness } = await import("@aexhq/brain/harness");
const { openCodeHarness } = await import("../dist/runtime.mjs");
await serveHarness(openCodeHarness());
