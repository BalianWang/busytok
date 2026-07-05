/**
 * Real Pi SDK session wrapper (Phase 3 Task 6).
 *
 * Wraps the `AgentSession` returned by `createAgentSession` from
 * `@earendil-works/pi-coding-agent` and adapts its event-driven API to the
 * synchronous `sendTurn() → result` shape the sidecar handler expects.
 *
 * Model resolution: per-session `AuthStorage.inMemory()` (sole source of
 * provider credentials — no env vars, no file I/O) + `ModelRegistry.create()`
 * with dynamic `registerProvider()` (spec §5.2). The resolved model is passed
 * into `createAgentSession({ model, authStorage, modelRegistry })` so the SDK
 * uses the configured provider/model instead of its default catalog.
 * Usage attribution (`usage.model`/`provider`) is sourced from the SDK's
 * `session.model` getter (the actual model used) with `resolvedProvider`
 * as fallback only.
 *
 * `SdkSession` is a minimal structural view of the methods we depend on so
 * tests can supply a fake without `vi.mock` and the type surface stays small.
 */
import {
  ERROR_CODE_AUTH_FAILURE,
  ERROR_CODE_RATE_LIMIT,
  ERROR_CODE_NETWORK,
  type ProviderKind,
} from './types.js';
import { SidecarError } from './errors.js';

/** Minimal structural view of `AgentSession` methods used by the wrapper. */
export interface SdkSession {
  readonly sessionId: string;
  /**
   * The current model the SDK resolved (verified against
   * `AgentSession.get model(): Model<any> | undefined`). Used for usage
   * attribution — sourced from the SDK result, not the requested option.
   */
  readonly model?: { readonly id: string; readonly provider: string } | undefined;
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
}

export interface SendTurnOptions {
  model?: string;
  provider_id?: string;
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
  model: string;
  provider_id: string;
  provider_kind: ProviderKind;
  provider_base_url: string;
  provider_api_key: string;
  model_reasoning: boolean;
  model_context_window: number;
  model_max_tokens: number;
  model_display_name?: string;
  tools?: string[];
}

/** Maps sidecar `ProviderKind` → Pi SDK `api` string (spec §5.2). */
const PROVIDER_KIND_TO_PI_API: Record<ProviderKind, string> = {
  openai_compatible: 'openai-completions',
  anthropic_compatible: 'anthropic-messages',
};

/**
 * Factory that creates a real `PiSdkSession`. Lazily imports the SDK so that
 * tests injecting a fake factory never load the (heavy) SDK module.
 *
 * Per-session provider/model registration (spec §5.2):
 * 1. `AuthStorage.inMemory()` — sole source of the provider API key (no env
 *    vars, no file I/O). Populated with `{ [provider_id]: { type: 'api_key', key } }`.
 * 2. `ModelRegistry.create(authStorage)` — in-memory registry, no file I/O.
 * 3. `registry.registerProvider(provider_id, { baseUrl, api, apiKey, models })` —
 *    dynamic provider with the requested model metadata.
 * 4. `registry.find(provider_id, model)` — precise model lookup; throws if not found.
 * 5. `createAgentSession({ model, authStorage, modelRegistry, cwd, tools })` —
 *    session bound to the registered provider/model.
 */
export const defaultSessionFactory = async (
  logical_subagent_id: string,
  opts: CreateSessionOpts,
): Promise<PiSdkSession> => {
  const { createAgentSession, ModelRegistry, AuthStorage } = await import('@earendil-works/pi-coding-agent');
  // 1. AuthStorage — in-memory, secret sole source.
  const authStorage = AuthStorage.inMemory({
    [opts.provider_id]: { type: 'api_key', key: opts.provider_api_key },
  });
  // 2. ModelRegistry — in-memory, no file I/O.
  const registry = ModelRegistry.create(authStorage);
  // 3. Dynamic provider registration.
  const piApi = PROVIDER_KIND_TO_PI_API[opts.provider_kind];
  registry.registerProvider(opts.provider_id, {
    baseUrl: opts.provider_base_url,
    api: piApi,
    apiKey: '__busytok_runtime__', // placeholder; real key from authStorage
    models: [{
      id: opts.model,
      name: opts.model_display_name ?? opts.model,
      reasoning: opts.model_reasoning,
      input: ['text'],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      contextWindow: opts.model_context_window,
      maxTokens: opts.model_max_tokens,
    }],
  });
  // 4. Precise model lookup.
  const model = registry.find(opts.provider_id, opts.model);
  if (!model) {
    throw new SidecarError(
      `model not found in registry after registerProvider: ${opts.model}`,
      -32603,
    );
  }
  // 5. Create session.
  const sessionOpts: {
    cwd: string;
    tools?: string[];
    model: unknown;
    authStorage: unknown;
    modelRegistry: unknown;
  } = {
    cwd: opts.cwd,
    model,
    authStorage,
    modelRegistry: registry,
    ...(opts.tools ? { tools: opts.tools } : {}),
  };
  const { session } = await createAgentSession(
    sessionOpts as Parameters<typeof createAgentSession>[0],
  );
  return new PiSdkSession(
    session as unknown as SdkSession,
    logical_subagent_id,
    session.sessionId,
    opts.provider_id,
    opts.model,
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
  private readonly resolvedProvider: string;
  /**
   * The model this session was created with (from `CreateSessionOpts.model`).
   * The hot pool compares this against `opts.model` on a hit to detect a
   * task-level `model_override` change and force a cold miss (P1-1).
   */
  readonly resolvedModel: string;
  private closed = false;

  constructor(
    sdk: SdkSession,
    logical_subagent_id: string,
    adapter_session_id: string,
    resolvedProvider: string,
    resolvedModel: string,
  ) {
    this.sdk = sdk;
    this.logical_subagent_id = logical_subagent_id;
    this.adapter_session_id = adapter_session_id;
    this.resolvedProvider = resolvedProvider;
    this.resolvedModel = resolvedModel;
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
    // Source model/provider from the SDK result (session.model — the actual
    // model the SDK used) with the requested option as fallback only.
    const sdkModel = this.sdk.model;
    return {
      status: 'completed',
      task_summary: text,
      usage: {
        model: sdkModel?.id ?? options.model ?? 'unknown',
        provider: sdkModel?.provider ?? this.resolvedProvider ?? options.provider_id ?? 'pi',
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
