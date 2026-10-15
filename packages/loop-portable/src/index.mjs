export function portable(configuration = {}) {
  if (configuration === null || typeof configuration !== "object" || Array.isArray(configuration) ||
      Object.keys(configuration).some(key => key !== "delegation") ||
      (configuration.delegation !== undefined && typeof configuration.delegation !== "boolean")) {
    throw new TypeError("portable accepts only the optional boolean delegation setting");
  }
  return { name: "portable", version: "1.0.0", configuration: { ...configuration } };
}
