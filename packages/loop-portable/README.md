# Portable loop

Choose this loop for a small, provider-neutral conversation that calls only
Brain-admitted models and actions. It runs outside the API process and checkpoints
each decision. A disconnected application does not stop managed work.

This is an unreleased source package for the matching Session API v2 source
runtime, not a published replacement for the existing native-loop packages.

```ts
import { portable } from "@aexhq/agentloop-portable";

const harness = portable();
// Pass harness in the Brain Session API v2 session configuration.
```

Use `portable({ delegation: true })` to let the model delegate one to eight tasks.
The loop supplies the `brain_delegate` model tool and returns a saved delegation or
fork decision; it does not start another process or call a provider itself.
Children inherit the session's configuration and enclosing limits. A task may supply
its own instructions, but cannot add permissions, credentials or budget.

The parent receives each original child's status and completion reference, including
partial failure or cancellation. These are untrusted observations, not instructions.
Large answers remain references when they cannot fit the bounded model context.
Neither an unavailable answer nor an unknown child effect is silently repeated.
An application action named `brain_delegate` cannot be shadowed when this option is enabled.

Operators install this package and register `brain-agentloop-portable`, version
`1.0.0`, with the configured OpenAI or Anthropic provider names. Registration is
server configuration; session callers cannot select executables.

The loop keeps at most 128 KiB of model context. New runs select a bounded recent
conversation; within a run, exceeding the bound fails explicitly rather than
dropping a pending tool call. Provider-native formatting stays in the model
selection. Structured-answer validation and corrective prompts belong to the
application, not this loop or the Brain kernel.

Continuation requires the exact checkpoint version and available recorded
operation outcomes. A missing or uncertain outcome never authorizes a new send.
This package does not claim hostile-code isolation or resumability of opaque
native processes.
