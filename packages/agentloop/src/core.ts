/**
 * The authoring core: `defineAgentloop` turns your handlers into the guest `activate` export,
 * driving every capability through typed `contracts/agentloop/v1` ctx operations.
 *
 * The host imports are late-bound: `buildLoopBundle` injects the canonical WIT context
 * bindings into the bundle entry, and tests bind a scripted host with `__bindHost`. Handlers
 * may be async — the guest runtime settles the returned promise
 * (every await resolves from host calls and microtasks alone; there is no ambient I/O).
 */

import type {
  AdmittedMessage,
  AgentloopErrorCode,
  AssistantMessage,
  JournalEntry,
  JournalEntryType,
  LoopEntry,
  ModelRequest,
  Seq,
  SessionContext,
  SessionStart,
  ToolCallRequest,
  ToolResult,
} from "./types.js";

let hostCall: ((operationId: string, payload: string) => string) | null = null;
let hostCancelled: (() => boolean) | null = null;

/**
 * Bind the host context. `buildLoopBundle` emits this for the real guest; tests bind
 * a scripted host. Calling ctx operations without a binding is a hard error, never a mock.
 */
export function __bindHost(
  call: (operationId: string, payload: string) => string,
  cancelled: () => boolean,
): void {
  hostCall = call;
  hostCancelled = cancelled;
}

/** A ctx operation the kernel answered with a typed error. Loops may catch and handle these. */
export class AgentloopOpError extends Error {
  readonly code: AgentloopErrorCode;
  readonly retryable: boolean;
  readonly details: Record<string, unknown> | undefined;

  constructor(error: {
    code: AgentloopErrorCode;
    message: string;
    retryable: boolean;
    details?: Record<string, unknown>;
  }) {
    super(error.message);
    this.name = "AgentloopOpError";
    this.code = error.code;
    this.retryable = error.retryable;
    this.details = error.details;
  }
}

let opCounter = 0;

function ctxOp(activationId: string, op: Record<string, unknown>): unknown {
  if (!hostCall) {
    throw new Error(
      "no loop host is bound; bundle this loop with buildLoopBundle, or bind a test host with __bindHost",
    );
  }
  const request = {
    op_id: `op-${++opCounter}`,
    activation_id: activationId,
    op,
  };
  const response = JSON.parse(hostCall(request.op_id, JSON.stringify(request))) as {
    result?: unknown;
    error?: {
      code: AgentloopErrorCode;
      message: string;
      retryable: boolean;
      details?: Record<string, unknown>;
    };
  };
  if (response.error) {
    throw new AgentloopOpError(response.error);
  }
  return response.result;
}

/** The per-turn capability surface. Every method journals through the kernel before its effect. */
export interface AgentloopCtx<Config = unknown> {
  /** The sealed session identity and kernel-enforced limits. */
  readonly session: SessionContext;
  /** The hydration this instance received at start, or null before the first session_start. */
  readonly start: SessionStart | null;
  /** Immutable session configuration supplied by the package factory. */
  readonly config: Config;
  /** True after Brain has cancelled or expired this activation. */
  readonly cancelled: () => boolean;
  readonly model: {
    /**
     * Execute one composed request against the session's sealed provider and model. Deltas
     * stream Brain-to-application directly; the folded message returns here.
     */
    stream(request: ModelRequest): Promise<AssistantMessage>;
  };
  readonly tools: {
    /** Dispatch calls against the sealed grant; results return in call order. */
    dispatch(calls: ToolCallRequest[]): Promise<ToolResult[]>;
  };
  readonly journal: {
    append(entries: LoopEntry[]): Promise<{ first_seq: Seq; last_seq: Seq }>;
    read(query?: {
      after_seq?: Seq;
      types?: JournalEntryType[];
      limit?: number;
    }): Promise<{ entries: JournalEntry[]; next_after_seq?: Seq }>;
  };
  readonly kv: {
    get(keys: string[]): Promise<Record<string, unknown>>;
    /** Key to JSON value; null deletes. Last writer wins per key. */
    set(entries: Record<string, unknown>): Promise<void>;
  };
  readonly turn: {
    /** Declare the turn finished, optionally with a structured result and a stop-reason
     * claim (`end_turn` when unstated; cancelled/interrupted stay kernel-owned). */
    finish(
      result?: Record<string, unknown>,
      options?: { stopReason?: "end_turn" | "max_rounds" | "refusal" },
    ): Promise<void>;
    fail(error: {
      message: string;
      code?: AgentloopErrorCode;
      retryable?: boolean;
    }): Promise<void>;
  };
}

export interface AgentloopHandlers<Config = unknown> {
  /** A fresh instance's hydration, before its first message. Rebuild in-memory state here. */
  onSessionStart?(start: SessionStart, session: SessionContext, config: Config): void | Promise<void>;
  /**
   * Drive one turn. Returning without `ctx.turn.finish`/`fail` finishes the turn; throwing
   * fails it with your error message.
   */
  onMessage(ctx: AgentloopCtx<Config>, message: AdmittedMessage): void | Promise<void>;
}

interface MessageActivation {
  kind: "message";
  activation_id: string;
  session: SessionContext;
  message: AdmittedMessage;
}

interface ComponentActivation {
  operationId: string;
  sessionId: string;
  kind: string;
  payloadJson: string;
  configJson: string;
  deadlineAtMs: bigint;
}

function makeCtx<Config>(
  activation: MessageActivation,
  start: SessionStart | null,
  config: Config,
): { ctx: AgentloopCtx<Config>; concluded: () => boolean } {
  const id = activation.activation_id;
  let concluded = false;
  const ctx: AgentloopCtx<Config> = {
    session: activation.session,
    start,
    config,
    cancelled: () => hostCancelled?.() ?? false,
    model: {
      async stream(request: ModelRequest): Promise<AssistantMessage> {
        const result = ctxOp(id, { op: "model_stream", request }) as {
          message: AssistantMessage;
        };
        return result.message;
      },
    },
    tools: {
      async dispatch(calls: ToolCallRequest[]): Promise<ToolResult[]> {
        const result = ctxOp(id, { op: "tools_dispatch", calls }) as {
          results: ToolResult[];
        };
        return result.results;
      },
    },
    journal: {
      async append(entries: LoopEntry[]): Promise<{ first_seq: Seq; last_seq: Seq }> {
        return ctxOp(id, { op: "journal_append", entries }) as {
          first_seq: Seq;
          last_seq: Seq;
        };
      },
      async read(query?: {
        after_seq?: Seq;
        types?: JournalEntryType[];
        limit?: number;
      }): Promise<{ entries: JournalEntry[]; next_after_seq?: Seq }> {
        return ctxOp(id, { op: "journal_read", ...query }) as {
          entries: JournalEntry[];
          next_after_seq?: Seq;
        };
      },
    },
    kv: {
      async get(keys: string[]): Promise<Record<string, unknown>> {
        const result = ctxOp(id, { op: "kv_get", keys }) as {
          entries: Record<string, unknown>;
        };
        return result.entries;
      },
      async set(entries: Record<string, unknown>): Promise<void> {
        ctxOp(id, { op: "kv_set", entries });
      },
    },
    turn: {
      async finish(
        result?: Record<string, unknown>,
        options?: { stopReason?: "end_turn" | "max_rounds" | "refusal" },
      ): Promise<void> {
        ctxOp(id, {
          op: "turn_finish",
          ...(result === undefined ? {} : { result }),
          ...(options?.stopReason === undefined ? {} : { stop_reason: options.stopReason }),
        });
        concluded = true;
      },
      async fail(error: {
        message: string;
        code?: AgentloopErrorCode;
        retryable?: boolean;
      }): Promise<void> {
        ctxOp(id, {
          op: "turn_fail",
          error: {
            code: error.code ?? "internal",
            message: error.message,
            retryable: error.retryable ?? false,
          },
        });
        concluded = true;
      },
    },
  };
  return { ctx, concluded: () => concluded };
}

/**
 * Turn handlers into the guest `activate` export:
 *
 * ```js
 * import { defineAgentloop } from "@aexhq/agentloop";
 * export const { activate } = defineAgentloop({
 *   async onMessage(ctx, message) {
 *     const round = await ctx.model.stream({ messages: [{ role: "user", content: message.content }] });
 *     await ctx.turn.finish();
 *   },
 * });
 * ```
 */
export function defineAgentloop<Config = unknown>(handlers: AgentloopHandlers<Config>): {
  activate(request: ComponentActivation): Promise<{ payloadJson: string }>;
} {
  let start: SessionStart | null = null;
  return {
    async activate(request: ComponentActivation): Promise<{ payloadJson: string }> {
      // Nothing may escape this export. The host compiles a thrown error into a bare Wasm trap
      // whose message is gone, so activation parsing and hydration report themselves too.
      let activationId = "act-unknown";
      try {
      const parsed = JSON.parse(request.payloadJson) as Record<string, unknown>;
      const config = JSON.parse(request.configJson) as Config;
      activationId = String(parsed["activation_id"] ?? "act-unknown");
      const completed = JSON.stringify({ activation_id: activationId, outcome: "completed" });
      if (request.kind === "session_start") {
        start = parsed as unknown as SessionStart;
        if (handlers.onSessionStart) {
          await handlers.onSessionStart(start, parsed["session"] as SessionContext, config);
        }
        return { payloadJson: completed };
      }
      if (request.kind !== "message") {
        return { payloadJson: completed };
      }
      const activation = parsed as unknown as MessageActivation;
      const { ctx, concluded } = makeCtx(activation, start, config);
      try {
        await handlers.onMessage(ctx, activation.message);
        if (!concluded()) {
          // Returning cleanly is finishing; the contract requires an explicit terminal, and a
          // return_direct tool may have committed one already (turn_already_terminal), which
          // is exactly the completed case.
          try {
            await ctx.turn.finish();
          } catch (error) {
            if (!(error instanceof AgentloopOpError && error.code === "turn_already_terminal")) {
              throw error;
            }
          }
        }
        return { payloadJson: completed };
      } catch (error) {
        if (error instanceof AgentloopOpError && error.code === "aborted") {
          return {
            payloadJson: JSON.stringify({
              activation_id: activationId,
              outcome: "aborted",
              error: {
                code: error.code,
                message: error.message,
                retryable: error.retryable,
                ...(error.details === undefined ? {} : { details: error.details }),
              },
            }),
          };
        }
        const message = error instanceof Error ? error.message : String(error);
        if (!concluded()) {
          try {
            await ctx.turn.fail({ message });
          } catch {
            // The kernel latch already owns the failure (e.g. the op channel is gone).
          }
        }
        return { payloadJson: failed(activationId, message) };
      }
      } catch (error) {
        return {
          payloadJson: failed(activationId, error instanceof Error ? error.message : String(error)),
        };
      }
    },
  };
}

const failed = (activationId: string, message: string): string => JSON.stringify({
  activation_id: activationId,
  outcome: "failed",
  error: { code: "internal", message: message.slice(0, 4096), retryable: false },
});
