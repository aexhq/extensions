import { component } from "@aexhq/brain";

export function app(options) {
  if (
    options === null ||
    typeof options !== "object" ||
    !/^[A-Za-z0-9_.:-]{1,128}$/u.test(options.id)
  ) {
    throw new TypeError(
      "app({ id }) requires 1 through 128 letters, digits, dots, colons, underscores, or hyphens",
    );
  }
  return component(
    "environment",
    new URL("./dist/environment.component.wasm", import.meta.url),
    { driver: "customer", configuration: { registration: options.id } },
    { metadata: { name: "app", source: "@aexhq/env-app" } },
  );
}
