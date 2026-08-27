export function awsMicrovm(options = {}) {
  if (options === null || typeof options !== "object" || Array.isArray(options)) {
    throw new TypeError("awsMicrovm options must be an object");
  }
  for (const name of Object.keys(options)) {
    if (!["id", "region", "idleSeconds", "maximumSeconds", "lifecyclePolicy"].includes(name)) {
      throw new TypeError(`unknown awsMicrovm option: ${name}`);
    }
  }
  const configuration = {};
  if (!/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u.test(options.id ?? "")) {
    throw new TypeError("awsMicrovm id must be a stable Environment identifier");
  }
  if (options.region !== undefined) {
    if (typeof options.region !== "string" || options.region.trim() === "") {
      throw new TypeError("awsMicrovm region must be a non-empty string");
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
  if (
    options.idleSeconds !== undefined &&
    options.maximumSeconds !== undefined &&
    options.idleSeconds > options.maximumSeconds
  ) {
    throw new TypeError("awsMicrovm idleSeconds cannot exceed maximumSeconds");
  }
  const lifecyclePolicy = options.lifecyclePolicy ?? "session";
  if (!["session", "shared", "external"].includes(lifecyclePolicy)) {
    throw new TypeError("awsMicrovm lifecyclePolicy must be session, shared, or external");
  }
  return Object.freeze({ environment_id: options.id, configuration: { driver: "aws-microvm", ...configuration }, lifecycle_policy: lifecyclePolicy });
}

function assertPositiveInteger(value, name) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new TypeError(`awsMicrovm ${name} must be a positive safe integer`);
  }
}
