export function finishedResult(result, sequence = 1) {
  return { call_id: result.call_id, sequence, finished: true, events: [
    { sequence: sequence + 1, event_type: "tool_result_emitted", data: { sequence, result } },
    { sequence: sequence + 2, event_type: "tool_call_ended", data: { sequence, outcome: { status: "ok", value: null } } },
  ] };
}
