import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { bash as factory } from "../dist/index.js";
import bash from "../dist/runtime/bash.mjs";
import { toolTests, workspace } from "../../../shared/tool-test-fixture.mjs";

toolTests("bash", factory, bash, { command: "" });

test("bash: runs in the workspace and preserves failure output", async (t) => {
  const context = await workspace(t);
  assert.deepEqual(await bash.execute({ command: "printf saved > result.txt; printf out; printf err >&2; exit 7" }, context), { exit_code: 7, stdout: "out", stderr: "err" });
  assert.equal(await readFile(join(context.workspace, "result.txt"), "utf8"), "saved");
});

test("bash: reports an aborted invocation", async (t) => {
  const context = await workspace(t);
  await assert.rejects(bash.execute({ command: "sleep 30" }, { ...context, signal: AbortSignal.abort() }), /abort/iu);
});
