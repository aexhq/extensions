import { callbacks, defineEnvironment } from "@aexhq/environment";

export const app = defineEnvironment({
  identity: "@aexhq/env-app",
  protocol: "environment/v1",
  profile: callbacks(),
  serialize(options) {
    if (options === null || typeof options !== "object" || !/^[A-Za-z0-9_.:-]{1,128}$/u.test(options.id)) {
      throw new TypeError("app({ id }) requires 1 through 128 letters, digits, dots, colons, underscores, or hyphens");
    }
    return { id: options.id };
  },
  handle() {
    return Object.freeze({ kind: "application-process" });
  },
});
