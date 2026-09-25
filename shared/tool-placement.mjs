// Presentation belongs to the Agentloop; execution always names an authorized pair.
export function toolPlacement(tools, { environmentSelection = "hidden", placements = {} } = {}) {
  const definitions = tools.filter(tool => tool.environments.length > 0).map(({ environments, environment_refs, ...definition }) => environmentSelection === "model" ? {
    ...definition,
    input_schema: {
      type: "object",
      properties: { environment: { type: "string", enum: environments }, input: nestedInput(definition.input_schema) },
      required: ["environment", "input"],
      additionalProperties: false,
    },
  } : definition);
  return {
    definitions,
    invocation(call) {
      const tool = tools.find((tool) => tool.name === call.name);
      if (!tool) throw new Error(`Tool ${call.name} is not authorized`);
      const visible = environmentSelection === "model";
      const environment = visible ? call.input?.environment : Object.hasOwn(placements, call.name) ? placements[call.name] : (tool.environments.length === 1 ? tool.environments[0] : undefined);
      if (!tool.environments.includes(environment)) throw new Error(`Tool ${call.name} requires an authorized Environment selection`);
      const reference = tool.environment_refs?.find(reference => reference.name === environment);
      return { ...call, environment, ...(reference === undefined ? {} : { environment_sequence: reference.sequence }), input: visible ? call.input.input : call.input };
    },
  };
}

// Local pointers still refer to the document root after a schema is nested.
function nestedInput(schema) {
  if (typeof schema === "boolean" || schema.$id !== undefined) return schema;
  const nested = { ...schema };
  for (const key of ["$ref", "$dynamicRef"]) {
    if (schema[key] === "#" || schema[key]?.startsWith("#/")) nested[key] = `#/properties/input${schema[key].slice(1)}`;
  }
  for (const key of ["additionalProperties", "unevaluatedProperties", "propertyNames", "contains", "contentSchema", "items", "unevaluatedItems", "not", "if", "then", "else"]) {
    if (schema[key] !== undefined) nested[key] = nestedInput(schema[key]);
  }
  for (const key of ["$defs", "definitions", "properties", "patternProperties", "dependentSchemas"]) {
    if (schema[key] !== undefined) nested[key] = Object.fromEntries(Object.entries(schema[key]).map(([name, value]) => [name, nestedInput(value)]));
  }
  for (const key of ["allOf", "anyOf", "oneOf", "prefixItems"]) {
    if (schema[key] !== undefined) nested[key] = schema[key].map(nestedInput);
  }
  return nested;
}

export async function refreshTools(tools, context) {
  const views = await context.environments.list();
  const controlled = new Set(views.flatMap(view => [view.reference.name, view.template]));
  return tools.map(tool => {
    const active = views.filter(view => !["deleted", "detached"].includes(view.state) && view.tools.some(definition => definition.name === tool.name));
    return { ...tool,
      environments: [...tool.environments.filter(name => !controlled.has(name)), ...active.map(view => view.reference.name)],
      environment_refs: [...(tool.environment_refs ?? []).filter(reference => !controlled.has(reference.name)), ...active.map(view => view.reference)],
    };
  });
}
