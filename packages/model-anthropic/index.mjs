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
    { provider: "anthropic", baseUrl: trimTrailingSlashes(baseUrl) },
    { metadata: { name: "anthropic", source: "@aexhq/model-anthropic" } },
  );
}

function trimTrailingSlashes(value) {
  let end = value.length;
  while (end > 0 && value.charCodeAt(end - 1) === 47) end -= 1;
  return value.slice(0, end);
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
