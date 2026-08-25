import { component } from "@aexhq/brain";

export function awsMicrovm(options = {}) {
  if (options === null || typeof options !== "object" || Array.isArray(options)) {
    throw new TypeError("awsMicrovm options must be an object");
  }
  const configuration = {};
  if (options.region !== undefined) {
    if (typeof options.region !== "string" || options.region.trim() === "") {
      throw new TypeError("awsMicrovm region must be a non-empty string");
    }
    configuration.region = options.region;
  }
  return component(
    "environment",
    new URL("./dist/environment.component.wasm", import.meta.url),
    { driver: "aws-microvm", configuration },
    { metadata: { name: "aws-microvm", source: "@aexhq/env-aws-microvm" } },
  );
}
