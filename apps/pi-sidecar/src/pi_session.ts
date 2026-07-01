/**
 * Real Pi SDK session wrapper (Phase 3 Task 6).
 *
 * Wraps the `AgentSession` returned by `createAgentSession` from
 * `@earendil-works/pi-coding-agent` and adapts its event-driven API to the
 * synchronous `sendTurn() → result` shape the sidecar handler expects.
 *
 * ---------------------------------------------------------------------------
 * SDK API deviations from the Task 6 brief (verified against
 * `@earendil-works/pi-coding-agent@0.80.2`):
 *
 * - Brief assumed `createAgentSession({ model: <string>, workingDir })`.
 *   Actual signature (dist/core/sdk.d.ts):
 *     `createAgentSession(options?: CreateAgentSessionOptions): Promise<CreateAgentSessionResult>`
 *   where `options.cwd` is the working directory (NOT `workingDir`), and
 *   `options.model` is a `Model<any>` object (NOT a string). Resolving a
 *   string model name to `Model<any>` requires `@earendil-works/pi-ai`'s
 *   `getModel`, which is out of scope for this task; we therefore omit
 *   `model` at creation time and let the SDK pick its default.
 * - Brief assumed `sendTurn(prompt, options)` returning a result. No such
 *   method exists. The real API is `prompt(text, options?): Promise<void>`
 *   (event-driven, resolves when the turn completes), with the assistant
 *   text read via `getLastAssistantText()` and usage via `getSessionStats()`.
 * - Brief assumed `close()`. The actual cleanup method is `dispose()`.
 *
 * `SdkSession` is a minimal structural view of the methods we depend on so
 * tests can supply a fake without `vi.mock` and the type surface stays small.
 */
import {
  ERROR_CODE_AUTH_FAILURE,
  ERROR_CODE_RATE_LIMIT,
  ERROR_CODE_NETWORK,
} from './types.js';
import { SidecarError } from './errors.js';

/** Minimal structural view of `AgentSession` methods used by the wrapper. */
export interface SdkSession {
  readonly sessionId: string;
  prompt(text: string): Promise<void>;
  getLastAssistantText(): string | undefined;
  getSessionStats(): SessionStatsLike;
  abort(): Promise<void>;
  dispose(): void;
}

export interface SessionStatsLike {
  tokens: {
    input: number;
    output: number;
    cacheRead: number;
    cacheWrite: number;
    total: number;
  };
  cost: number;
  model?: unknown;
}

export interface SendTurnOptions {
  model?: string;
  tools?: string[];
  timeout_ms?: number;
}

export interface SendTurnResult {
  status: 'completed' | 'failed' | 'timeout';
  task_summary: string;
  usage: {
    model: string;
    provider: string;
    input_tokens: number;
    output_tokens: number;
    cache_read_tokens: number;
    cache_write_tokens: number;
    cost_usd: number;
  };
}

/** Options passed through to the session factory at creation time. */
export interface CreateSessionOpts {
  cwd: string;
  model?: string;
  tools?: string[];
}

/**
 * Factory that creates a real `PiSdkSession`. Lazily imports the SDK so that
 * tests injecting a fake factory never load the (heavy) SDK module.
 */
export const defaultSessionFactory = async (
  logical_subagent_id: string,
  opts: CreateSessionOpts,
): Promise<PiSdkSession> => {
  const { createAgentSession } = await import('@earendil-works/pi-coding-agent');
  const { session } = await createAgentSession({
    cwd: opts.cwd,
    ...(opts.tools ? { tools: opts.tools } : {}),
  });
  return new PiSdkSession(
    session as unknown as SdkSession,
    logical_subagent_id,
    session.sessionId,
  );
};

export type SessionFactory = typeof defaultSessionFactory;

/**
 * Wraps an SDK `AgentSession`, tracking the metadata the hot pool needs and
 * exposing a synchronous `sendTurn()` that classifies SDK errors into the
 * sidecar's JSON-RPC error codes.
 */
export class PiSdkSession {
  readonly adapter_session_id: string;
  readonly logical_subagent_id: string;
  readonly created_at_ms: number;
  last_used_at_ms: number;
  private readonly sdk: SdkSession;
  private closed = false;

  constructor(
    sdk: SdkSession,
    logical_subagent_id: string,
    adapter_session_id: string,
  ) {
    this.sdk = sdk;
    this.logical_subagent_id = logical_subagent_id;
    this.adapter_session_id = adapter_session_id;
    const now = Date.now();
    this.created_at_ms = now;
    this.last_used_at_ms = now;
  }

  /** Send a prompt and map the SDK response to a TurnAutoResult-compatible shape. */
  async sendTurn(promptText: string, options: SendTurnOptions = {}): Promise<SendTurnResult> {
    if (this.closed) {
      throw new SidecarError('session is closed', -32001);
    }
    this.last_used_at_ms = Date.now();

    const timeoutMs = options.timeout_ms;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let timedOut = false;
    const timeoutPromise = new Promise<never>((_, reject) => {
      if (timeoutMs && timeoutMs > 0) {
        timer = setTimeout(() => {
          timedOut = true;
          // Abort the in-flight turn so prompt() settles promptly.
          this.sdk.abort().catch(() => {});
          reject(new Error('turn timeout'));
        }, timeoutMs);
      }
    });

    try {
      await Promise.race([this.sdk.prompt(promptText), timeoutPromise]);
    } catch (err) {
      throw classifyError(err, timedOut);
    } finally {
      if (timer) clearTimeout(timer);
    }

    const text = this.sdk.getLastAssistantText() ?? '';
    const stats = this.sdk.getSessionStats();
    return {
      status: 'completed',
      task_summary: text,
      usage: {
        model: options.model ?? String(stats.model ?? 'unknown'),
        provider: 'pi',
        input_tokens: stats.tokens.input,
        output_tokens: stats.tokens.output,
        cache_read_tokens: stats.tokens.cacheRead,
        cache_write_tokens: stats.tokens.cacheWrite,
        cost_usd: stats.cost,
      },
    };
  }

  /** Dispose the underlying SDK session. Safe to call once. */
  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    try {
      this.sdk.dispose();
    } catch {
      // Ignore disposal errors during cleanup.
    }
  }

  /** Whether `close()` has been called. */
  isClosed(): boolean {
    return this.closed;
  }
}

/**
 * Classify an SDK error into a `SidecarError` with the appropriate JSON-RPC code.
 * - 401/403 → auth (-32010), 429 → rate_limit (-32011), network → network (-32012)
 * - timeouts (timedOut flag or AbortError) → -32003 (TASK_TIMEOUT)
 * - anything else → -32603 (internal error)
 */
function classifyError(err: unknown, timedOut: boolean): SidecarError {
  if (timedOut) {
    return new SidecarError('turn timed out', -32003);
  }
  const message = err instanceof Error ? err.message : String(err);
  const status = getStatus(err);
  if (status === 401 || status === 403) {
    return new SidecarError(`auth failure: ${message}`, ERROR_CODE_AUTH_FAILURE);
  }
  if (status === 429) {
    return new SidecarError(`rate limit: ${message}`, ERROR_CODE_RATE_LIMIT);
  }
  if (isNetworkError(err)) {
    return new SidecarError(`network error: ${message}`, ERROR_CODE_NETWORK);
  }
  if (isAbortError(err)) {
    return new SidecarError('turn timed out', -32003);
  }
  return new SidecarError(message, -32603);
}

function getStatus(err: unknown): number | undefined {
  if (!err || typeof err !== 'object') return undefined;
  const e = err as Record<string, unknown>;
  if (typeof e.status === 'number') return e.status;
  if (typeof e.statusCode === 'number') return e.statusCode;
  const response = e.response as Record<string, unknown> | undefined;
  if (response && typeof response.status === 'number') return response.status;
  return undefined;
}

const NETWORK_CODES = new Set([
  'ENOTFOUND',
  'ECONNREFUSED',
  'ECONNRESET',
  'EAI_AGAIN',
  'EHOSTUNREACH',
  'ENETUNREACH',
  'ETIMEDOUT',
  'EPIPE',
]);

function isNetworkError(err: unknown): boolean {
  if (!(err instanceof Error)) return false;
  const code = (err as { code?: unknown }).code;
  if (typeof code === 'string' && NETWORK_CODES.has(code)) return true;
  // fetch() throws TypeError for network-level failures.
  if (err instanceof TypeError && /fetch|network|socket/i.test(err.message)) {
    return true;
  }
  return false;
}

function isAbortError(err: unknown): boolean {
  if (!(err instanceof Error)) return false;
  const name = (err as { name?: unknown }).name;
  const code = (err as { code?: unknown }).code;
  return name === 'AbortError' || code === 'ABORT_ERR' || name === 'TimeoutError';
}
