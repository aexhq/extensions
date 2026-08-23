import { computer, defineEnvironment, linux } from "@aexhq/environment";

export const awsMicrovm = defineEnvironment({
  identity: "@aexhq/env-aws-microvm",
  protocol: "environment/v1",
  profile: computer({ platform: linux.arm64, network: "allowlist", recovery: "retained" }),
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
    const generation = async () => {
      let status = await context.request("GET", base);
      if (status?.generation === undefined) status = await context.request("POST", base);
      if (typeof status?.generation !== "string" || status.generation === "") {
        throw new TypeError("AWS MicroVM environment did not return a generation");
      }
      return status.generation;
    };
    return Object.freeze({
      status: () => context.request("GET", base),
      files: Object.freeze({
        list: async (path = ".") => context.request("POST", `${base}/files/list`, {
          path: workspacePath(path),
          generation: await generation(),
        }),
        read: async (path) => {
          const result = await context.request("POST", `${base}/files/read-inline`, {
            path: workspacePath(path),
            generation: await generation(),
          });
          if (typeof result?.content_base64 !== "string") {
            throw new TypeError("AWS MicroVM environment returned invalid file content");
          }
          return Uint8Array.from(atob(result.content_base64), (character) => character.charCodeAt(0));
        },
        upload: async (path, content, options = {}) => {
          const bytes = typeof content === "string" ? new TextEncoder().encode(content) : content;
          if (!(bytes instanceof Uint8Array)) throw new TypeError("File content must be a string or Uint8Array");
          if (bytes.byteLength > 1024 * 1024) {
            throw new TypeError("Inline AWS MicroVM uploads are limited to 1048576 bytes");
          }
          let binary = "";
          for (const byte of bytes) binary += String.fromCharCode(byte);
          return context.request("POST", `${base}/files/write-inline`, {
            path: workspacePath(path),
            generation: await generation(),
            content_base64: btoa(binary),
            ...(options.overwrite === undefined ? {} : { overwrite: options.overwrite }),
          });
        },
      }),
    });
  },
});

function workspacePath(path) {
  if (typeof path !== "string" || path === "" || path.includes("\0")) {
    throw new TypeError("File path must be a non-empty string");
  }
  return path.startsWith("/") ? path : `/workspace/${path === "." ? "" : path}`;
}
