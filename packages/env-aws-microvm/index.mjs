import { defineEnvironment } from "@aexhq/brain";

export function awsMicroVm(options = {}) {
  if (options === null || typeof options !== "object" || Array.isArray(options)) {
    throw new TypeError("awsMicroVm options must be an object");
  }
  for (const name of Object.keys(options)) {
    if (!["id", "region", "idleSeconds", "maximumSeconds", "lifecycle"].includes(name)) {
      throw new TypeError(`unknown awsMicroVm option: ${name}`);
    }
  }
  const lifecycle = options.lifecycle ?? "session";
  if (!["session", "shared", "external"].includes(lifecycle)) {
    throw new TypeError("awsMicroVm lifecycle must be session, shared, or external");
  }
  if (lifecycle === "session" && options.id !== undefined) {
    throw new TypeError("a session awsMicroVm cannot declare an id");
  }
  if (lifecycle !== "session" && !validIdentifier(options.id)) {
    throw new TypeError("a shared or external awsMicroVm requires a stable id");
  }
  const configuration = { driver: "aws-microvm" };
  if (options.region !== undefined) {
    if (typeof options.region !== "string" || options.region.trim() === "") {
      throw new TypeError("awsMicroVm region must be a non-empty string");
    }
    configuration.region = options.region;
  }
  if (options.idleSeconds !== undefined) {
    assertPositiveInteger(options.idleSeconds, "idleSeconds");
    configuration.idle_seconds = options.idleSeconds;
  }
  if (options.maximumSeconds !== undefined) {
    assertPositiveInteger(options.maximumSeconds, "maximumSeconds");
    configuration.maximum_seconds = options.maximumSeconds;
  }
  if (options.idleSeconds !== undefined && options.maximumSeconds !== undefined && options.idleSeconds > options.maximumSeconds) {
    throw new TypeError("awsMicroVm idleSeconds cannot exceed maximumSeconds");
  }
  return defineEnvironment({
    capability: "workspace",
    configuration,
    lifecycle: lifecycle === "session" ? {} : { type: lifecycle, id: options.id },
  });
}

function assertPositiveInteger(value, name) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new TypeError(`awsMicroVm ${name} must be a positive safe integer`);
  }
}

function validIdentifier(value) {
  return typeof value === "string" && /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u.test(value);
}
