import { computer, defineEnvironment, linux } from "@aexhq/environment";

export const awsMicrovm = defineEnvironment({
  identity: "@aexhq/env-aws-microvm",
  protocol: "environment/v1",
  profile: computer({ platform: linux.amd64, network: "allowlist", recovery: "retained" }),
  serialize(options = {}) {
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
    return configuration;
  },
  handle(context) {
    const base = `/v1/sessions/${encodeURIComponent(context.sessionId)}/environments/${encodeURIComponent(context.environment)}`;
    return Object.freeze({
      status: () => context.request("GET", base),
      files: Object.freeze({
        list: (path = ".") => context.request("GET", `${base}/files?path=${encodeURIComponent(path)}`),
        read: (path) => context.request("GET", `${base}/files/content?path=${encodeURIComponent(path)}`),
        write: (path, contentBase64) => context.request("POST", `${base}/files`, { path, content_base64: contentBase64 }),
      }),
    });
  },
});
