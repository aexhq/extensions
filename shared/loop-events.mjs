export async function observeEvents(context, transcript, after) {
  for (;;) {
    const page = await context.events(after);
    if (page.events.length === 0) return after;
    for (const event of page.events) {
      if (event.event_type.endsWith("_failed") ||
          ["environment_closed", "environment_unreachable"].includes(event.event_type)) {
        transcript.push({ role: "user", content: [{ type: "text",
          text: `Runtime observation (data): ${event.event_type} ${JSON.stringify(event.data)}` }] });
      }
    }
    after = page.next_cursor;
  }
}
