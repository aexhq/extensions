/**
 * The Agentloop component builder (node-only; import from `@aexhq/agentloop/build`). The SDK
 * is bundled from its canonical TypeScript source, then an explicitly supplied compiler emits
 * the publishable Wasm component. Brain never compiles extension source.
 */

import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build, type Plugin } from "esbuild";

/**
 * Build provenance for the official compiler. Runtime identity is the component SHA-256 plus
 * the canonical Agentloop WIT digest, independent of the compiler that produced the bytes.
 */
export const LOOP_TOOLCHAIN = "componentize-js-0.22.0";

/** The authoring bundle bound before component compilation. */
export const MAX_LOOP_BUNDLE_BYTES = 8 * 1024 * 1024;

export interface LoopBundle {
  /** The complete ESM source bundle, the exact bytes to upload. */
  source: string;
  /** SHA-256 of the UTF-8 source bytes for reproducible build provenance. */
  sha256: string;
  bytes: number;
}

export interface AgentloopComponent extends LoopBundle {
  component: Uint8Array;
  componentSha256: string;
  componentBytes: number;
}

export type AgentloopCompiler = (
  source: string,
  wit: string,
  options: { worldName: "agentloop"; disableFeatures: readonly ["http", "fetch-event"] },
) => Promise<{ component: Uint8Array }>;

export interface BuildLoopBundleOptions {
  /** The loop entry module exporting `activate` (typically via `defineAgentloop`). */
  entry: string;
  /**
   * Skip the Unicode-property-escape gate. The pinned guest engine rejects `\p{…}` in regex
   * literals at parse time, so a bundle carrying one fails at session create; only set this
   * when every occurrence is provably inside a plain string.
   */
  allowUnicodePropertyEscapes?: boolean;
  /**
   * esbuild plugins applied while bundling — the hook for dependency compatibility rewrites
   * (e.g. replacing a library's `\p{…}` regex literals with engine-parseable equivalents).
   * Plugins shape the deterministic source bundle, so they are part of what the sealed
   * digest covers.
   */
  plugins?: Plugin[];
  /** Modules injected before the bundle's own top-level code (esbuild `inject`) — polyfills. */
  inject?: string[];
}

const SDK_ENTRY = fileURLToPath(new URL("../src/index.ts", import.meta.url));

export async function buildLoopBundle(options: BuildLoopBundleOptions): Promise<LoopBundle> {
  const entryPath = resolve(options.entry);
  // The virtual entry binds the real host import around the author's module, so authored
  // loops never import the WIT host interface themselves and stay unit-testable in node.
  const virtualEntry = [
    'import { call, cancelled } from "aex:agentloop/context@1.0.0";',
    'import { __bindHost } from "@aexhq/agentloop";',
    "__bindHost(call, cancelled);",
    `export { activate } from ${JSON.stringify(entryPath.replaceAll("\\", "/"))};`,
    "",
  ].join("\n");
  const bundled = await build({
    stdin: {
      contents: virtualEntry,
      resolveDir: dirname(entryPath),
      loader: "js",
      sourcefile: "agentloop-entry.js",
    },
    bundle: true,
    format: "esm",
    platform: "neutral",
    external: ["aex:agentloop/context@1.0.0"],
    alias: { "@aexhq/agentloop": SDK_ENTRY },
    plugins: options.plugins ?? [],
    inject: options.inject ?? [],
    write: false,
    legalComments: "none",
  });
  const output = bundled.outputFiles[0];
  if (!output) {
    throw new Error("esbuild produced no output for the loop bundle");
  }
  const source = output.text;
  lintLoopBundle(source, options);
  const bytes = Buffer.byteLength(source, "utf8");
  if (bytes > MAX_LOOP_BUNDLE_BYTES) {
    throw new Error(`the loop bundle is ${bytes} bytes; the upload bound is 8 MiB`);
  }
  const sha256 = createHash("sha256").update(source, "utf8").digest("hex");
  return { source, sha256, bytes };
}

export async function buildAgentloopComponent(
  options: BuildLoopBundleOptions,
  compiler: AgentloopCompiler,
): Promise<AgentloopComponent> {
  if (typeof compiler !== "function") {
    throw new TypeError("buildAgentloopComponent requires an explicit component compiler");
  }
  const bundle = await buildLoopBundle(options);
  const wit = await readFile(
    new URL(import.meta.resolve("@aexhq/brain/contracts/agentloop")),
    "utf8",
  );
  const output = await compiler(bundle.source, wit, {
    worldName: "agentloop",
    disableFeatures: ["http", "fetch-event"],
  });
  const component = new Uint8Array(output.component);
  return {
    ...bundle,
    component,
    componentSha256: createHash("sha256").update(component).digest("hex"),
    componentBytes: component.byteLength,
  };
}

/**
 * Refuse bundles that would fail at guest parse time. `\p{…}`/`\P{…}` in a regex literal is a
 * SyntaxError in the pinned engine; occurrences inside plain strings are indistinguishable
 * without a full parse, so the gate is strict with an explicit override.
 */
export function lintLoopBundle(
  source: string,
  options?: Pick<BuildLoopBundleOptions, "allowUnicodePropertyEscapes">,
): void {
  if (options?.allowUnicodePropertyEscapes) {
    return;
  }
  const offenders: number[] = [];
  const lines = source.split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    if (/\\[pP]\{/.test(lines[index] ?? "")) {
      offenders.push(index + 1);
    }
  }
  if (offenders.length > 0) {
    throw new Error(
      `the bundle contains Unicode property escapes (\\p{…}) on line(s) ${offenders
        .slice(0, 8)
        .join(", ")}; the guest engine rejects them in regex literals at parse time. ` +
        "Rewrite the pattern with explicit ranges, or pass allowUnicodePropertyEscapes: true " +
        "if every occurrence is provably inside a plain string.",
    );
  }
}
