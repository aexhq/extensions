import { DatabaseSync } from "node:sqlite";
import { createHash, timingSafeEqual } from "node:crypto";
import { tool } from "@aexhq/brain";
import { z } from "zod";
import { createToolHandler } from "@aexhq/env-http/handler";

export function recordsApplication(filename, token) {
  const db = new DatabaseSync(filename);
  db.exec(`CREATE TABLE IF NOT EXISTS members(company TEXT, subject TEXT, role TEXT, active INTEGER, PRIMARY KEY(company,subject));
    CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY, company TEXT, subject TEXT);
    CREATE TABLE IF NOT EXISTS records(id TEXT PRIMARY KEY, company TEXT, title TEXT, version INTEGER);
    CREATE TABLE IF NOT EXISTS saves(session TEXT, operation TEXT, proposal TEXT, receipt TEXT, PRIMARY KEY(session,operation));
    CREATE TABLE IF NOT EXISTS reports(id TEXT PRIMARY KEY, session TEXT, status TEXT, result TEXT);`);
  const member = (company, subject) => {
    const value = db.prepare("SELECT role FROM members WHERE company=? AND subject=? AND active=1").get(company, subject);
    if (!value) throw new Error("access denied");
    return value;
  };
  const owner = session => {
    const value = db.prepare("SELECT company,subject FROM sessions WHERE id=?").get(session);
    if (!value) throw new Error("unknown session");
    member(value.company, value.subject);
    return value;
  };
  function readRecord(company, subject, id) {
    member(company, subject);
    const value = db.prepare("SELECT id,title,version FROM records WHERE id=? AND company=?").get(id, company);
    if (!value) throw new Error("record not found");
    return { ...value };
  }
  const title = z.string().min(1).max(200);
  const record = z.object({ id: z.string(), title, version: z.number().int() });
  const proposal = z.strictObject({ id: z.string(), title, version: z.number().int() });
  const tools = [
    tool({ name: "read_record", description: "Read one authorized record", input: z.object({ id: z.string() }), output: record,
      run: ({ id }, call) => { const user = owner(call.sessionId); return readRecord(user.company, user.subject, id); } })(),
    tool({ name: "propose_title", description: "Prepare a title change for user review; does not save", input: z.object({ id: z.string(), title }), output: proposal,
      run: ({ id, title }, call) => { const user = owner(call.sessionId); return { ...readRecord(user.company, user.subject, id), title }; } })(),
    tool({ name: "start_report", description: "Submit a report job; does not wait for completion", input: z.object({}),
      run: (_, call) => {
        owner(call.sessionId);
        const id = `${call.sessionId}:${call.sequence}`;
        db.prepare("INSERT INTO reports VALUES(?,?,'queued',NULL) ON CONFLICT DO NOTHING").run(id, call.sessionId);
        return { id };
      } })(),
    tool({ name: "get_report", description: "Inspect an authorized report job", input: z.object({ id: z.string() }),
      run: ({ id }, call) => {
        owner(call.sessionId);
        const job = db.prepare("SELECT id,status,result FROM reports WHERE id=? AND session=?").get(id, call.sessionId);
        if (!job) throw new Error("report not found");
        return { ...job };
      } })(),
  ];
  const hash = value => createHash("sha256").update(value).digest();
  const handler = createToolHandler({ tools, authorize: (request, call) => {
    if (!timingSafeEqual(hash(request.headers.get("authorization") ?? ""), hash(`Bearer ${token}`))) throw new Error("denied");
    owner(call.sessionId);
  } });
  function saveReviewed(subject, session, operation, raw) {
    const change = proposal.parse(raw);
    db.exec("BEGIN IMMEDIATE");
    try {
      const user = owner(session);
      if (user.subject !== subject || member(user.company, subject).role !== "editor") throw new Error("save denied");
      const document = JSON.stringify(change);
      const saved = db.prepare("SELECT proposal,receipt FROM saves WHERE session=? AND operation=?").get(session, operation);
      let receipt;
      if (saved) {
        if (saved.proposal !== document) throw new Error("operation key changed");
        receipt = JSON.parse(saved.receipt);
      } else {
        const update = db.prepare("UPDATE records SET title=?,version=version+1 WHERE id=? AND company=? AND version=?").run(change.title, change.id, user.company, change.version);
        if (update.changes !== 1) throw new Error("record changed; review again");
        receipt = { operation, record: readRecord(user.company, subject, change.id) };
        db.prepare("INSERT INTO saves VALUES(?,?,?,?)").run(session, operation, document, JSON.stringify(receipt));
      }
      db.exec("COMMIT");
      return receipt;
    } catch (error) { db.exec("ROLLBACK"); throw error; }
  }
  return { db, tools, handler, readRecord, saveReviewed, close: () => db.close() };
}
