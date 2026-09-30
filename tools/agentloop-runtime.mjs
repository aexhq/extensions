import * as host from "brain:agentloop/host@0.2.0";

export async function turn(input) {
  try {
    const { source, configuration } = JSON.parse(input.configurationJson);
    if (typeof source !== "string") throw new TypeError("program source is required");
    const program = new Function("require", `${source}\nreturn brainProgram;`)(specifier => {
      if (specifier !== "brain:agentloop/host@0.2.0") throw new TypeError(`unsupported program import: ${specifier}`);
      return host;
    });
    return await program.turn({ ...input, configurationJson: JSON.stringify(configuration) });
  } catch (error) {
    if (error?.payload) throw error;
    const failure = new Error(String(error?.message ?? error));
    failure.payload = { code: "agentloop_failed", message: failure.message, retryable: false };
    throw failure;
  }
}
