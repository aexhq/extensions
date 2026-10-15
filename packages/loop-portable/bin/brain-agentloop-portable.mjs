#!/usr/bin/env node
import { once } from "node:events";
import { decide } from "../src/decision.mjs";

let pending = Buffer.alloc(0);
try {
  for await (const chunk of process.stdin) {
    pending = Buffer.concat([pending, chunk]);
    if (pending.length > 1048576) throw new Error("request exceeds one MiB");
    let newline;
    while ((newline = pending.indexOf(10)) !== -1) {
      const line = pending.subarray(0, newline);
      pending = pending.subarray(newline + 1);
      const result = JSON.stringify(decide(JSON.parse(line)));
      if (Buffer.byteLength(result) > 262000) throw new Error("decision exceeds output limit");
      if (!process.stdout.write(`${result}\n`)) await once(process.stdout, "drain");
    }
  }
  if (pending.length !== 0) throw new Error("incomplete decision request");
} catch {
  process.stderr.write("portable decision failed\n");
  process.exitCode = 1;
  process.stdin.destroy();
}
