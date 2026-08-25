import { component } from "@aexhq/brain";

export function openai(options = {}) {
  if (options === null || typeof options !== "object" || Array.isArray(options)) {
    throw new TypeError("openai options must be an object");
  }
  const baseUrl = options.baseUrl ?? "https://api.openai.com";
  if (!isHttpsUrl(baseUrl, options.allowHttp === true)) {
    throw new TypeError("openai baseUrl must be an absolute HTTPS URL");
  }
  if (options.outputTokenParameter !== undefined &&
      options.outputTokenParameter !== "max_tokens" &&
      options.outputTokenParameter !== "max_completion_tokens") {
    throw new TypeError("openai outputTokenParameter is invalid");
  }
  return component(
    "model",
    new URL("./dist/model.component.wasm", import.meta.url),
    {
      provider: "openai",
      baseUrl: trimTrailingSlashes(baseUrl),
      outputTokenParameter: options.outputTokenParameter ?? "max_completion_tokens",
    },
    { metadata: { name: "openai", source: "@aexhq/model-openai" } },
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
