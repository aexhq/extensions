#!/usr/bin/env node
import { serveHarness } from "@aexhq/brain/harness";
import { codexHarness } from "../dist/runtime.mjs";
await serveHarness(codexHarness());
