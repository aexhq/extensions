import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { StdioClientTransport } from "@modelcontextprotocol/client/stdio";
import { connectMcp } from "../dist/index.mjs";

export async function mcpFixture(t) {
  const directory = await mkdtemp(join(tmpdir(), "aex-mcp-test-"));
  let client;
  t.after(async () => { try { await client?.close(); } finally { await rm(directory, { recursive: true }); } });
  const journal = join(directory, "calls.jsonl");
  await writeFile(journal, "");
  const transport = new StdioClientTransport({ command: process.execPath,
    args: [fileURLToPath(new URL("./server.mjs", import.meta.url)), journal], stderr: "inherit" });
  client = await connectMcp(transport, { versionNegotiation: { mode: { pin: "2026-07-28" } }, capabilities: { elicitation: { form: {} } } });
  return { client, records: async () => (await readFile(journal, "utf8")).trim().split("\n").filter(Boolean).map(JSON.parse) };
}
