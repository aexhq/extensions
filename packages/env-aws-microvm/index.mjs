import { component } from "@aexhq/brain";

export function awsMicrovm(options = {}) {
  if (options === null || typeof options !== "object" || Array.isArray(options)) {
    throw new TypeError("awsMicrovm options must be an object");
  }
  for (const name of Object.keys(options)) {
    if (!["region", "idleSeconds", "maximumSeconds"].includes(name)) {
      throw new TypeError(`unknown awsMicrovm option: ${name}`);
    }
  }
  const configuration = {};
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
  return component(
    "environment",
    new URL("./dist/environment.component.wasm", import.meta.url),
    { driver: "aws-microvm", configuration },
    { metadata: { name: "aws-microvm", source: "@aexhq/env-aws-microvm" } },
  );
}

function assertPositiveInteger(value, name) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new TypeError(`awsMicrovm ${name} must be a positive safe integer`);
  }
}
