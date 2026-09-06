// Presentation belongs to the Agentloop; execution always names an authorized pair.
export function toolPlacement(tools, { environmentSelection = "hidden", placements = {} } = {}) {
  const definitions = tools.map(({ environments, ...definition }) => environmentSelection === "model" ? {
    ...definition,
    input_schema: {
      type: "object",
      properties: { environment: { type: "string", enum: environments }, input: definition.input_schema },
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
      return { ...call, environment, input: visible ? call.input.input : call.input };
    },
  };
}
