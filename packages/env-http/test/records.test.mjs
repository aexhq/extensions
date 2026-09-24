import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { inspectTool } from "@aexhq/brain";
import { recordsApplication } from "../examples/records.mjs";

test("fresh handlers share authorized records and jobs; only a reviewed atomic save mutates the record", async t => {
  const directory = await mkdtemp(join(tmpdir(), "http-records-"));
  t.after(() => rm(directory, { recursive: true }));
  const file = join(directory, "app.sqlite");
  const open = () => recordsApplication(file, "secret");
  let app = open();
  app.db.exec("INSERT INTO members VALUES('company','alice','editor',1),('other','bob','editor',1); INSERT INTO sessions VALUES('session','company','alice'),('foreign','other','bob'); INSERT INTO records VALUES('record','company','Old title',1)");
  app.close();
  async function invoke(name, input, sequence, sessionId = "session") {
    const app = open();
    try {
      const placed = app.tools.find(t => inspectTool(t).definition.name === name);
      return (await app.handler(new Request("https://app.example/tools", { method: "POST", headers: { authorization: "Bearer secret" }, body: JSON.stringify({
        contract: "http-tool/v1", sessionId, environment: "app", sequence, tool: inspectTool(placed).definition, input, deadlineAtMs: Date.now()+5000,
      }) }))).json();
    } finally { app.close(); }
  }
  const proposed = await invoke("propose_title", { id: "record", title: "New title" }, 1);
  assert.equal(proposed.type, "success");
  assert.equal((await invoke("read_record", { id: "record" }, 2)).value.title, "Old title");
  assert.equal((await invoke("read_record", { id: "record" }, 3, "foreign")).type, "failure");
  app = open();
  assert.throws(() => app.saveReviewed("bob", "session", "save", proposed.value), /denied/u);
  const saved = app.saveReviewed("alice", "session", "save", proposed.value);
  assert.deepEqual(app.saveReviewed("alice", "session", "save", proposed.value), saved);
  assert.throws(() => app.saveReviewed("alice", "session", "save", { ...proposed.value, title: "different" }), /key changed/u);
  assert.throws(() => app.saveReviewed("alice", "session", "stale", proposed.value), /review again/u);
  app.close();
  assert.equal((await invoke("read_record", { id: "record" }, 4)).value.version, 2);
  const job = (await invoke("start_report", {}, 5)).value;
  assert.deepEqual((await invoke("start_report", {}, 5)).value, job);
  app = open();
  app.db.prepare("UPDATE reports SET status='cancelled' WHERE id=?").run(job.id);
  app.close();
  assert.deepEqual(await invoke("get_report", job, 6), { type: "success", value: { id: job.id, status: "cancelled", result: null } });
  app = open();
  app.db.exec("UPDATE members SET active=0 WHERE subject='alice'");
  app.close();
  assert.equal((await invoke("get_report", job, 7)).code, "denied");
});
