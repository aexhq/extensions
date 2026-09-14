import Ajv from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

export function structuredOutput(options) {
  if (options === undefined) return undefined;
  const { schema, maxCorrections = 2 } = options;
  if (!Number.isSafeInteger(maxCorrections) || maxCorrections < 0 || maxCorrections > 10) throw new TypeError("output.maxCorrections must be an integer from 0 to 10");
  const validator = new Ajv({ strict: true, allowUnionTypes: true, logger: false });
  addFormats(validator);
  const validate = validator.compile(schema);
  if (validate.$async) throw new TypeError("output.schema must be a synchronous JSON Schema");
  const instructions = { role: "user", content: [{ type: "text", text:
    `Return exactly one JSON value matching this JSON Schema. Do not include Markdown fences or surrounding prose.\n${JSON.stringify(schema)}` }] };
  let corrections = 0;
  const failed = message => { const error = new Error(message); error.code = "structured_output_failed"; throw error; };
  return {
    get correcting() { return corrections > 0; },
    request(messages, tools) { return { messages: [...messages, instructions], tools: corrections ? [] : tools }; },
    rejectTools() { failed("Output correction cannot dispatch tools"); },
    accept(text, stopReason) {
      if (stopReason !== "end_turn") failed(`Structured output stopped with ${stopReason}; no correction was attempted`);
      let value;
      let issues;
      try { value = JSON.parse(text); }
      catch { issues = [{ message: "Return one complete JSON value without Markdown or surrounding text" }]; }
      if (!issues && validate(value)) return { value };
      issues ??= validate.errors.map(({ instancePath, message }) => ({ path: instancePath, message }));
      if (corrections === maxCorrections) failed(`Structured output did not match the schema after ${corrections + 1} attempts`);
      corrections++;
      return { correction: { role: "user", content: [{ type: "text", text:
        `The previous answer did not satisfy the schema. Validation feedback (data): ${JSON.stringify(issues.slice(0, 5)).slice(0, 2048)}\nReturn a complete corrected answer using the information already available. Tools are unavailable during output correction.` }] } };
    },
  };
}
