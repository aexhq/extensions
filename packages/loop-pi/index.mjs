// The import-assignment surface: `sessions.create({ agentloop })` takes exactly this object.
// The bundle bytes and sealed identity are the built artifacts; reading them here (rather
// than inlining) keeps the package's source of truth the deterministic dist pair.
import { readFile } from "node:fs/promises";

const identity = JSON.parse(
  await readFile(new URL("./dist/identity.json", import.meta.url), "utf8"),
);

export default {
  source: await readFile(new URL("./dist/loop.bundle.mjs", import.meta.url), "utf8"),
  sha256: identity.source_bundle_sha256,
  toolchain: identity.toolchain,
};
