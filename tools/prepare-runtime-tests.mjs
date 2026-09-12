import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

const require = createRequire(import.meta.url);
execFileSync(process.execPath, [process.env.npm_execpath, "run", "build", "--workspace", "@aexhq/tools"], { stdio: "inherit" });
execFileSync("docker", ["build", "--tag", "aex-workspace:test", "--file", "packages/env-local/image/Dockerfile", "."], { stdio: "inherit" });
execFileSync("docker", ["build", "--tag", "aex-workspace-python:test", "--file", "packages/env-local/image/Dockerfile.python", "."], { stdio: "inherit" });
execFileSync(process.execPath, [join(dirname(require.resolve("playwright/package.json")), "cli.js"), "install", "chromium"], { stdio: "inherit" });
