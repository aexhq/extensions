import { component } from "@aexhq/brain";

export function anthropic(options = {}) {
  if (options === null || typeof options !== "object" || Array.isArray(options)) {
    throw new TypeError("anthropic options must be an object");
  }
  const baseUrl = options.baseUrl ?? "https://api.anthropic.com";
  if (!isHttpsUrl(baseUrl, options.allowHttp === true)) {
    throw new TypeError("anthropic baseUrl must be an absolute HTTPS URL");
  }
  return component(
    "model",
    new URL("./dist/model.component.wasm", import.meta.url),
    { provider: "anthropic", baseUrl: baseUrl.replace(/\/+$/u, "") },
    { metadata: { name: "anthropic", source: "@aexhq/model-anthropic" } },
  );
}

function isHttpsUrl(value, allowHttp) {
  try {
    const url = new URL(value);
    return url.username === "" && url.password === "" && url.search === "" && url.hash === "" &&
      (url.protocol === "https:" || (allowHttp && url.protocol === "http:"));
  } catch {
    return false;
  }
}
