import { readFile } from "node:fs/promises";
import { execFile } from "node:child_process";

try {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  const { implementation, input } = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  let output;
  if (implementation.type === "aex_official_tool") {
    if (!/^(bash|read|write|edit|ls|glob|grep|todo)$/u.test(implementation.name)) throw new Error("unsupported Tool");
    const runtime = (await import(`/opt/aex/tools/${implementation.name}.mjs`)).default;
    output = await runtime.execute(input, { workspace: "/workspace", signal: new AbortController().signal });
  } else if (implementation.type === "python_project") {
    const projects = JSON.parse(await readFile("/opt/aex/python-projects.json", "utf8"));
    if (!Object.hasOwn(projects, implementation.name)) throw new Error("unsupported Python project");
    const project = projects[implementation.name];
    output = await new Promise((resolve, reject) => {
      const child = execFile(`${project.directory}/.venv/bin/python`, ["-m", project.module],
        { cwd: "/workspace", env: { ...process.env, PYTHONPATH: project.directory, PYTHONDONTWRITEBYTECODE: "1" },
          maxBuffer: 16 * 1024 * 1024 }, (error, stdout) => {
          if (error) reject(error);
          else { try { resolve(JSON.parse(stdout)); } catch (failure) { reject(failure); } }
        });
      child.stdin.on("error", reject);
      child.stdin.end(JSON.stringify(input));
    });
  } else throw new Error("unsupported implementation");
  process.stdout.write(JSON.stringify({ type: "result", output }));
} catch (error) {
  process.stdout.write(JSON.stringify({ type: "failure", code: "tool_error", message: String(error.message).slice(0, 4096), retryable: false }));
}
