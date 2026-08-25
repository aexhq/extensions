const DEFAULT_MAX_FRAME = 256 * 1024;
const MAX_EVENTS_PER_FEED = 4096;

export class SseDecoder {
  #buffer = new Uint8Array();
  #maxFrame;

  constructor(maxFrame = DEFAULT_MAX_FRAME) {
    if (!Number.isSafeInteger(maxFrame) || maxFrame < 1) {
      throw new TypeError("maxFrame must be a positive safe integer");
    }
    this.#maxFrame = maxFrame;
  }

  get pending() {
    return this.#buffer.byteLength;
  }

  feed(chunk) {
    if (!(chunk instanceof Uint8Array)) throw new TypeError("SSE chunks must be Uint8Array values");
    const combined = new Uint8Array(this.#buffer.byteLength + chunk.byteLength);
    combined.set(this.#buffer);
    combined.set(chunk, this.#buffer.byteLength);
    const events = [];
    let start = 0;
    for (let index = 0; index < combined.byteLength; index += 1) {
      let end = -1;
      let next = index + 1;
      if (combined[index] === 10 && combined[index + 1] === 10) {
        end = index;
        next = index + 2;
      } else if (
        combined[index] === 13 && combined[index + 1] === 10 &&
        combined[index + 2] === 13 && combined[index + 3] === 10
      ) {
        end = index;
        next = index + 4;
      }
      if (end < 0) continue;
      if (end - start > this.#maxFrame) throw new Error(`SSE frame exceeded ${this.#maxFrame} bytes`);
      const event = parseFrame(combined.subarray(start, end));
      if (event !== undefined) {
        if (events.length >= MAX_EVENTS_PER_FEED) {
          throw new Error(`provider chunk contained more than ${MAX_EVENTS_PER_FEED} SSE events`);
        }
        events.push(event);
      }
      start = next;
      index = next - 1;
    }
    this.#buffer = combined.slice(start);
    if (this.#buffer.byteLength > this.#maxFrame) {
      throw new Error(`SSE frame exceeded ${this.#maxFrame} bytes without a terminator`);
    }
    return events;
  }
}

export function parseJson(value, field) {
  try {
    return JSON.parse(value);
  } catch (error) {
    throw new Error(`${field} is not valid JSON: ${error.message}`);
  }
}

export function usage(fields) {
  return Object.fromEntries(Object.entries(fields).filter(([, value]) => value !== undefined));
}

export function terminal(stopReason) {
  return JSON.stringify({ stopReason: stopReason ?? "unknown" });
}

function parseFrame(bytes) {
  const source = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  let event;
  const data = [];
  for (const rawLine of source.split(/\r?\n/u)) {
    const line = rawLine.endsWith("\r") ? rawLine.slice(0, -1) : rawLine;
    if (line === "" || line.startsWith(":")) continue;
    const separator = line.indexOf(":");
    const field = separator < 0 ? line : line.slice(0, separator);
    let value = separator < 0 ? "" : line.slice(separator + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    if (field === "event") event = value;
    if (field === "data") data.push(value);
  }
  if (event === undefined && data.length === 0) return undefined;
  return { event, data: data.join("\n") };
}
