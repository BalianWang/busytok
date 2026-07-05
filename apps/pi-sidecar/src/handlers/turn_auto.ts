import { type TurnAutoParams, type TurnAutoResult } from '../types.js';
import type { RequestHandler } from '../rpc.js';
import { SidecarError } from '../errors.js';
import type { SessionPool } from '../session_pool.js';
import { PiSdkSession, type SdkSession, type SessionFactory, type CreateSessionOpts } from '../pi_session.js';

// Mock session id generator (used only when BUSYTOK_USE_MOCK_SIDECAR=1).
let sessionCounter = 0;
function nextMockSessionId(): string {
  sessionCounter++;
  return `pi_sess_mock_${sessionCounter}`;
}

/**
 * Mock factory: produces a no-op `PiSdkSession` whose `sendTurn` is never
 * invoked (mockTurnAuto returns hardcoded usage). Exists purely so the pool's
 * reuse/limit logic can run in mock mode without touching the real SDK.
 */
const mockSessionFactory: SessionFactory = async (logical_subagent_id, opts) => {
  const sid = nextMockSessionId();
  // `opts.model` is threaded as `resolvedModel` so the pool's model-mismatch
  // cold-miss (P1-1) works in mock mode too — a changed `model_override`
  // evicts the old mock session and creates a fresh one.
  return new PiSdkSession(noopSdkSession(sid), logical_subagent_id, sid, 'mock', opts.model);
};

function noopSdkSession(id: string): SdkSession {
  return {
    sessionId: id,
    prompt: async () => {},
    getLastAssistantText: () => '',
    getSessionStats: () => ({
      tokens: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
      cost: 0,
    }),
    abort: async () => {},
    dispose: () => {},
  };
}

/**
 * turn_auto handler factory — takes a SessionPool so the pool is shared
 * across requests. Routes to the real SDK path by default, or to the mock
 * path when `BUSYTOK_USE_MOCK_SIDECAR=1` (keeps e2e tests working without
 * real Pi credentials).
 */
export function turnAutoHandlerWithPool(pool: SessionPool): RequestHandler {
  return async (params) => {
    const p = params as TurnAutoParams;
    if (!p.logical_subagent_id || !p.prompt) {
      throw new SidecarError('missing required fields', -32602);
    }
    const useMock = process.env.BUSYTOK_USE_MOCK_SIDECAR === '1';
    if (useMock) {
      return mockTurnAuto(p, pool);
    }
    return realTurnAuto(p, pool);
  };
}

/**
 * Build `CreateSessionOpts` from `TurnAutoParams`. Used by both the mock and
 * real paths so the pool always receives a complete opts object (the hit
 * branch ignores it per spec §5.5; the miss branch threads it to the factory).
 *
 * `model` is required on `TurnAutoParams` (M-5: tightened from optional) — no
 * fallback needed. The mock path supplies a real model value via the params.
 */
function buildSessionOpts(p: TurnAutoParams): CreateSessionOpts {
  const opts: CreateSessionOpts = {
    cwd: p.cwd,
    model: p.model,
    provider_id: p.provider_id,
    provider_kind: p.provider_kind,
    provider_base_url: p.provider_base_url,
    provider_api_key: p.provider_api_key,
    model_reasoning: p.model_reasoning,
    model_context_window: p.model_context_window,
    model_max_tokens: p.model_max_tokens,
    ...(p.model_display_name ? { model_display_name: p.model_display_name } : {}),
    ...(p.tools ? { tools: p.tools } : {}),
  };
  return opts;
}

/**
 * Mock path — preserves the Phase 1 mock behavior (hardcoded usage, mock
 * adapter_session_ids) so Rust-side e2e tests run without credentials.
 */
async function mockTurnAuto(p: TurnAutoParams, pool: SessionPool): Promise<TurnAutoResult> {
  const { session, reused } = await pool.ensure(
    p.logical_subagent_id,
    buildSessionOpts(p),
    mockSessionFactory,
  );
  const now = Date.now();
  const memoryUpdate = process.env.BUSYTOK_MOCK_MEMORY_UPDATE === '1'
    ? {
        current_state_summary: 'Investigated context; produced memory update.',
        key_files: [{ path: 'src/auth/token.ts', reason: 'refresh logic', last_seen_at_ms: now, score: 3 }],
        decisions: ['Focus on read-only analysis'],
        open_questions: [{ question: 'Concurrent refresh handled?', status: 'open' as const, created_at_ms: now, last_seen_at_ms: now }],
      }
    : undefined;
  // task_summary is a REAL summary, NOT an echo of compact_context.
  // The bash mock fixture (mock-sidecar.sh) handles the echo for e2e tests.
  return {
    adapter_session_id: session.adapter_session_id,
    session_reused: reused,
    status: 'completed',
    result: {
      task_summary: `[mock] turn completed for: ${p.prompt.slice(0, 80)}`,
      ...(memoryUpdate ? { memory_update: memoryUpdate } : {}),
    },
    usage: {
      model: p.model,
      provider: 'deepseek',
      input_tokens: p.prompt.length,
      output_tokens: 50,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      cost_usd: 0.001,
    },
  };
}

/**
 * Real path — drives the Pi SDK session via `sendTurn` and maps the result to
 * the sidecar's `TurnAutoResult`. SDK errors are classified into JSON-RPC
 * error codes by `PiSdkSession.sendTurn` (auth/rate-limit/network/timeout).
 */
async function realTurnAuto(p: TurnAutoParams, pool: SessionPool): Promise<TurnAutoResult> {
  // M-5: `model` is now required on `TurnAutoParams` (tightened from optional).
  // The dead `if (!p.model) throw` guard is removed — TypeScript enforces the
  // contract at compile time, and the Rust side always sends the bound model.
  const { session, reused } = await pool.ensure(
    p.logical_subagent_id,
    buildSessionOpts(p),
  );
  const result = await session.sendTurn(p.prompt, {
    model: p.model,
    provider_id: p.provider_id,
    tools: p.tools,
    timeout_ms: p.timeout_ms,
  });
  return {
    adapter_session_id: session.adapter_session_id,
    session_reused: reused,
    status: result.status,
    result: {
      task_summary: result.task_summary,
    },
    usage: result.usage,
  };
}
