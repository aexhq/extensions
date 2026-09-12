export function validateMedia(item) {
  if (!item || !((item.type === "image" && Object.keys(item).every(key => ["type", "url"].includes(key)))
    || (item.type === "file" && item.media_type === "application/pdf" && Object.keys(item).every(key => ["type", "url", "media_type"].includes(key))))
    || typeof item.url !== "string") throw new TypeError("media must contain Brain image or PDF URL blocks");
  const url = new URL(item.url);
  if (url.protocol !== "https:" || url.username || url.password) throw new TypeError("media requires HTTPS without credentials");
  return item;
}

export async function publish(publishMedia, bytes, mediaType, index, context) {
  if (typeof publishMedia !== "function") throw new TypeError("publishMedia is required for model media");
  if (!["image/png", "image/jpeg", "image/gif", "image/webp", "application/pdf"].includes(mediaType)) throw new TypeError(`unsupported media type: ${mediaType}`);
  context.signal.throwIfAborted();
  const url = await publishMedia({ bytes, mediaType, index }, context);
  context.signal.throwIfAborted();
  return validateMedia(mediaType === "application/pdf" ? { type: "file", media_type: mediaType, url } : { type: "image", url });
}
