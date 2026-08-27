import { componentize } from "@bytecodealliance/componentize-js";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { build } from "esbuild";

export const LOOP_TOOLCHAIN = "componentize-js-0.19.3";

export interface BuildAgentloopOptions {
  entry: string;
  out?: string;
  wit?: string;
}

export interface AgentloopPackage {
  manifest: {
    contract_version: "agentloop/v1";
    component_digest: string;
    component_bytes: number;
    toolchain: string;
  };
  component_base64: string;
}

export async function buildAgentloop(options: BuildAgentloopOptions): Promise<AgentloopPackage> {
  const entry = resolve(options.entry);
  const source = wrapper(entry);
  const bundled = await build({
    stdin: { contents: source, resolveDir: dirname(entry), sourcefile: "brain-agentloop-entry.js", loader: "js" },
    bundle: true,
    format: "esm",
    platform: "neutral",
    write: false,
    legalComments: "none",
  });
  const output = bundled.outputFiles[0];
  if (!output) throw new Error("esbuild produced no Agentloop output");
  const wit = options.wit ?? await readFile(new URL(import.meta.resolve("@aexhq/brain/contracts/agentloop")), "utf8");
  const work = await mkdtemp(join(tmpdir(), "brain-agentloop-"));
  let component: Uint8Array;
  try {
    const sourcePath = join(work, "agentloop.js");
    const witPath = join(work, "agentloop.wit");
    await Promise.all([writeFile(sourcePath, output.text), writeFile(witPath, wit)]);
    const compiled = await componentize({
      sourcePath,
      witPath,
      worldName: "agentloop",
      disableFeatures: ["stdio", "random", "clocks", "http", "fetch-event"],
    });
    component = new Uint8Array(compiled.component);
  } finally {
    await rm(work, { recursive: true, force: true });
  }
  const componentDigest = createHash("sha256").update(component).digest("hex");
  const packageValue: AgentloopPackage = {
    manifest: { contract_version: "agentloop/v1", component_digest: componentDigest, component_bytes: component.byteLength, toolchain: LOOP_TOOLCHAIN },
    component_base64: Buffer.from(component).toString("base64"),
  };
  if (options.out !== undefined) await writeFile(options.out, `${JSON.stringify(packageValue)}\n`);
  return packageValue;
}

function wrapper(entry: string): string {
  const normalized = entry.replaceAll("\\", "/");
  return `
import agentloop from ${JSON.stringify(normalized)};

const decodeObservation = (observation) => {
  switch (observation.tag) {
    case "session-started": return { type: "session_started" };
    case "user-message": return { type: "user_message", content: JSON.parse(observation.val) };
    case "model-completed": return { type: "model_completed", response: JSON.parse(observation.val) };
    case "tools-completed": return { type: "tools_completed", results: JSON.parse(observation.val) };
    case "emitted": return { type: "emitted", event: JSON.parse(observation.val) };
    case "cancelled": return { type: "cancelled" };
    default: throw new Error("unknown observation " + observation.tag);
  }
};

const encodeDecision = (decision) => {
  switch (decision.type) {
    case "model": return { tag: "model", val: JSON.stringify(decision.request) };
    case "tools": return { tag: "tools", val: decision.calls.map((call) => ({ callId: call.callId, name: call.name, inputJson: JSON.stringify(call.input) })) };
    case "emit": return { tag: "emit", val: JSON.stringify(decision.event) };
    case "finish": return { tag: "finish", val: decision.result === undefined ? undefined : JSON.stringify(decision.result) };
    case "fail": return { tag: "fail", val: [decision.code, decision.message, decision.retryable ?? false] };
    default: throw new Error("unknown decision " + decision.type);
  }
};

export function step(input) {
  const output = agentloop.step({
    context: { protocolVersion: input.context.protocolVersion, items: JSON.parse(input.context.itemsJson), ...(input.context.stateJson === undefined ? {} : { state: JSON.parse(input.context.stateJson) }) },
    observation: decodeObservation(input.observation),
    presentation: input.presentation,
    runtime: input.runtime,
  });
  if (output && typeof output.then === "function") throw new Error("Agentloop step must be synchronous");
  return {
    context: { protocolVersion: output.context.protocolVersion, itemsJson: JSON.stringify(output.context.items), stateJson: output.context.state === undefined ? undefined : JSON.stringify(output.context.state) },
    decision: encodeDecision(output.decision),
  };
}
`;
}
