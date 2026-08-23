import assert from "node:assert/strict";
import test from "node:test";
import { inspectEnvironment } from "@aexhq/environment";
import { awsMicrovm } from "./index.mjs";

test("awsMicrovm declares the computer profile without a language runtime", () => {
  assert.deepEqual(inspectEnvironment(awsMicrovm()).serialized, {
    extension: "@aexhq/env-aws-microvm",
    protocol: "environment/v1",
    profile: { kind: "computer", platform: "linux-arm64", network: "allowlist", recovery: "retained" },
    configuration: {},
  });
});

test("awsMicrovm handle scopes lifecycle and files to its logical environment", async () => {
  const calls = [];
  const reference = awsMicrovm();
  const handle = inspectEnvironment(reference).createHandle({
    sessionId: "ses_1",
    environment: "workspace",
    async request(method, path, body) {
      calls.push({ method, path, body });
      if (method === "GET") return { state: "running", generation: "gen_1" };
      if (path.endsWith("/files/read-inline")) return { content_base64: "aGk=" };
      return { path: body?.path };
    },
  });
  await handle.files.upload("input.txt", "hi");
  assert.deepEqual(await handle.files.read("input.txt"), new TextEncoder().encode("hi"));
  assert.deepEqual(calls[1], {
    method: "POST",
    path: "/v1/sessions/ses_1/environments/workspace/files/write-inline",
    body: {
      path: "/workspace/input.txt",
      generation: "gen_1",
      content_base64: "aGk=",
    },
  });
  assert.equal(calls.every(({ path }) => path.includes("/environments/workspace")), true);
});
