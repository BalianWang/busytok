import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { SessionPool } from '../src/session_pool.js';
import { SidecarError } from '../src/errors.js';
import { turnAutoHandlerWithPool } from '../src/handlers/turn_auto.js';
import { PiSdkSession, type SdkSession, type SessionFactory, type SessionStatsLike } from '../src/pi_session.js';
import { ERROR_CODE_TURN_CANCELLED } from '../src/types.js';

/**
 * Real-SDK-path tests for turn_auto. The real `createAgentSession` is NOT
 * called here; instead we inject a fake `SessionFactory` into the pool that
 * returns `PiSdkSession` instances wrapping a controllable `SdkSession` stub.
 * This exercises the full `realTurnAuto → sendTurn → classifyError` path
 * (including error classification) without network/credentials.
 */

interface FakeConfig {
  /** If set, prompt() throws an Error carrying these fields. */
  promptError?: { status?: number; code?: string; message?: string };
  /** Assistant text returned by getLastAssistantText(). */
  assistantText?: string;
  /** Stats returned by getSessionStats(). */
  stats?: Partial<SessionStatsLike>;
  /**
   * The model the SDK reports via the `session.model` getter (for usage
   * attribution). Defaults to `{ id: 'deepseek-chat', provider: 'deepseek' }`.
   */
  sdkModel?: { id: string; provider: string };
  /** If true, prompt() hangs until abort() is called (for timeout tests). */
  hang?: boolean;
}

function makeFakeSdk(id: string, config: FakeConfig): SdkSession {
  let abortResolve: () => void = () => {};
  const abortPromise = new Promise<void>((r) => { abortResolve = r; });
  return {
    sessionId: id,
    model: 'sdkModel' in config ? config.sdkModel : { id: 'deepseek-chat', provider: 'deepseek' },
    prompt: async () => {
      if (config.hang) {
        await abortPromise;
        return;
      }
      if (config.promptError) {
        const pe = config.promptError;
        const err = Object.assign(new Error(pe.message ?? 'sdk error'), {
          ...(pe.status !== undefined ? { status: pe.status } : {}),
          ...(pe.code ? { code: pe.code } : {}),
        });
        throw err;
      }
    },
    getLastAssistantText: () => config.assistantText ?? 'real assistant summary',
    getSessionStats: (): SessionStatsLike => ({
      tokens: {
        input: config.stats?.tokens?.input ?? 42,
        output: config.stats?.tokens?.output ?? 88,
        cacheRead: config.stats?.tokens?.cacheRead ?? 5,
        cacheWrite: config.stats?.tokens?.cacheWrite ?? 7,
        total: config.stats?.tokens?.total ?? 142,
      },
      cost: config.stats?.cost ?? 0.0099,
    }),
    abort: async () => { abortResolve(); },
    dispose: () => {},
  };
}

/** Factory that yields PiSdkSessions wrapping fake SDK sessions per `config`. */
function fakeFactory(config: FakeConfig = {}): SessionFactory {
  let n = 0;
  return async (subagent: string, opts) => {
    const id = `fake_${++n}`;
    return new PiSdkSession(makeFakeSdk(id, config), subagent, id, 'test-provider', opts.model);
  };
}

// Ensure the real path is exercised (not the mock path).
const PREV_MOCK = process.env.BUSYTOK_USE_MOCK_SIDECAR;
beforeAll(() => { delete process.env.BUSYTOK_USE_MOCK_SIDECAR; });
afterAll(() => {
  if (PREV_MOCK !== undefined) process.env.BUSYTOK_USE_MOCK_SIDECAR = PREV_MOCK;
});

const BASE_PARAMS = {
  logical_subagent_id: 'sub-1',
  cwd: '/tmp',
  profile: 'pi/default',
  prompt: 'refactor the auth module',
  model: 'deepseek-chat',
};

describe('turn_auto real SDK path', () => {
  it('maps a successful SDK turn to TurnAutoResult with real usage', async () => {
    const pool = new SessionPool(3, fakeFactory({
      assistantText: 'refactored auth module successfully',
      stats: { tokens: { input: 42, output: 88, cacheRead: 5, cacheWrite: 7, total: 142 }, cost: 0.0099, model: 'deepseek-chat' } },
    ));
    const handler = turnAutoHandlerWithPool(pool);
    const result = await handler(BASE_PARAMS) as {
      status: string; adapter_session_id: string; session_reused: boolean;
      result: { task_summary: string };
      usage: { input_tokens: number; output_tokens: number; cache_read_tokens: number; cache_write_tokens: number; cost_usd: number; model: string; provider: string };
    };
    expect(result.status).toBe('completed');
    expect(result.session_reused).toBe(false);
    expect(result.adapter_session_id).toMatch(/^fake_/);
    expect(result.result.task_summary).toBe('refactored auth module successfully');
    expect(result.usage.input_tokens).toBe(42);
    expect(result.usage.output_tokens).toBe(88);
    expect(result.usage.cache_read_tokens).toBe(5);
    expect(result.usage.cache_write_tokens).toBe(7);
    expect(result.usage.cost_usd).toBe(0.0099);
    expect(result.usage.model).toBe('deepseek-chat');
    expect(result.usage.provider).toBe('deepseek');
  });

  it('classifies a 401 SDK error as auth failure (-32010)', async () => {
    const pool = new SessionPool(3, fakeFactory({
      promptError: { status: 401, message: 'Unauthorized' },
    }));
    const handler = turnAutoHandlerWithPool(pool);
    await expect(handler(BASE_PARAMS)).rejects.toThrow(SidecarError);
    try {
      await handler({ ...BASE_PARAMS, logical_subagent_id: 'sub-401' });
    } catch (e) {
      expect((e as SidecarError).code).toBe(-32010);
      expect((e as SidecarError).message).toContain('auth failure');
    }
  });

  it('classifies a 403 SDK error as auth failure (-32010)', async () => {
    const pool = new SessionPool(3, fakeFactory({
      promptError: { status: 403, message: 'Forbidden' },
    }));
    const handler = turnAutoHandlerWithPool(pool);
    try {
      await handler({ ...BASE_PARAMS, logical_subagent_id: 'sub-403' });
    } catch (e) {
      expect((e as SidecarError).code).toBe(-32010);
    }
  });

  it('classifies a 429 SDK error as rate limit (-32011)', async () => {
    const pool = new SessionPool(3, fakeFactory({
      promptError: { status: 429, message: 'Too Many Requests' },
    }));
    const handler = turnAutoHandlerWithPool(pool);
    await expect(handler(BASE_PARAMS)).rejects.toThrow(SidecarError);
    try {
      await handler({ ...BASE_PARAMS, logical_subagent_id: 'sub-429' });
    } catch (e) {
      expect((e as SidecarError).code).toBe(-32011);
      expect((e as SidecarError).message).toContain('rate limit');
    }
  });

  it('classifies a network error as network (-32012)', async () => {
    const pool = new SessionPool(3, fakeFactory({
      promptError: { code: 'ECONNREFUSED', message: 'connect ECONNREFUSED' },
    }));
    const handler = turnAutoHandlerWithPool(pool);
    try {
      await handler({ ...BASE_PARAMS, logical_subagent_id: 'sub-net' });
    } catch (e) {
      expect((e as SidecarError).code).toBe(-32012);
      expect((e as SidecarError).message).toContain('network error');
    }
  });

  it('classifies a turn timeout as TASK_TIMEOUT (-32003)', async () => {
    const pool = new SessionPool(3, fakeFactory({ hang: true }));
    const handler = turnAutoHandlerWithPool(pool);
    try {
      await handler({ ...BASE_PARAMS, logical_subagent_id: 'sub-timeout', timeout_ms: 50 });
    } catch (e) {
      expect((e as SidecarError).code).toBe(-32003);
      expect((e as SidecarError).message).toContain('timed out');
    }
  });

  it('falls back to mock path when BUSYTOK_USE_MOCK_SIDECAR=1', async () => {
    const pool = new SessionPool(3); // real default factory, but mock path bypasses it
    const handler = turnAutoHandlerWithPool(pool);
    const prev = process.env.BUSYTOK_USE_MOCK_SIDECAR;
    process.env.BUSYTOK_USE_MOCK_SIDECAR = '1';
    try {
      const result = await handler(BASE_PARAMS) as {
        adapter_session_id: string; usage: { input_tokens: number; output_tokens: number };
      };
      expect(result.adapter_session_id).toMatch(/^pi_sess_mock_/);
      expect(result.usage.input_tokens).toBe(BASE_PARAMS.prompt.length);
      expect(result.usage.output_tokens).toBe(50);
    } finally {
      if (prev === undefined) delete process.env.BUSYTOK_USE_MOCK_SIDECAR;
      else process.env.BUSYTOK_USE_MOCK_SIDECAR = prev;
    }
  });

  it('reuses the SDK session for the same logical_subagent_id', async () => {
    const pool = new SessionPool(3, fakeFactory());
    const handler = turnAutoHandlerWithPool(pool);
    const r1 = await handler(BASE_PARAMS) as { adapter_session_id: string; session_reused: boolean };
    const r2 = await handler({ ...BASE_PARAMS, prompt: 'second turn' }) as {
      adapter_session_id: string; session_reused: boolean;
    };
    expect(r1.session_reused).toBe(false);
    expect(r2.session_reused).toBe(true);
    expect(r2.adapter_session_id).toBe(r1.adapter_session_id);
  });

  it('throws HOT_SESSION_LIMIT_REACHED (-32002) with candidate when pool is full', async () => {
    const pool = new SessionPool(1, fakeFactory());
    const handler = turnAutoHandlerWithPool(pool);
    await handler(BASE_PARAMS);
    await expect(
      handler({ ...BASE_PARAMS, logical_subagent_id: 'sub-2' }),
    ).rejects.toThrow(SidecarError);
    try {
      await handler({ ...BASE_PARAMS, logical_subagent_id: 'sub-2' });
    } catch (e) {
      expect((e as SidecarError).code).toBe(-32002);
      expect((e as SidecarError).data?.candidate).toMatch(/^fake_/);
    }
  });
});

describe('PiSdkSession unit', () => {
  it('sendTurn returns completed status and maps usage from SDK model', async () => {
    const sdk = makeFakeSdk('s1', {
      assistantText: 'done',
      stats: { tokens: { input: 1, output: 2, cacheRead: 3, cacheWrite: 4, total: 10 }, cost: 0.5 },
      sdkModel: { id: 'm', provider: 'test-provider' },
    });
    const session = new PiSdkSession(sdk, 'sub', 's1', 'test-provider', 'deepseek-chat');
    const result = await session.sendTurn('hi', { model: 'fallback-model' });
    expect(result.status).toBe('completed');
    expect(result.task_summary).toBe('done');
    // model/provider sourced from SDK's session.model, NOT options.model
    expect(result.usage).toMatchObject({
      input_tokens: 1, output_tokens: 2, cache_read_tokens: 3, cache_write_tokens: 4, cost_usd: 0.5, model: 'm', provider: 'test-provider',
    });
  });

  it('sendTurn falls back to options.model when SDK model is undefined', async () => {
    const sdk = makeFakeSdk('s1', {
      assistantText: 'done',
      sdkModel: undefined,
    });
    // Override the default model by removing it
    (sdk as { model?: unknown }).model = undefined;
    const session = new PiSdkSession(sdk, 'sub', 's1', 'resolved-provider', 'deepseek-chat');
    const result = await session.sendTurn('hi', { model: 'fallback-model', provider_id: 'pid' });
    expect(result.usage.model).toBe('fallback-model');
    expect(result.usage.provider).toBe('resolved-provider');
  });

  it('close() is idempotent and marks the session closed', async () => {
    const sdk = makeFakeSdk('s2', {});
    const session = new PiSdkSession(sdk, 'sub', 's2', 'test-provider', 'deepseek-chat');
    await session.close();
    await session.close();
    expect(session.isClosed()).toBe(true);
    await expect(session.sendTurn('x')).rejects.toThrow(SidecarError);
  });
});

// --- Ghost session cleanup (P0 fix) ---
//
// When turn_auto creates a new session (not reused) and the turn fails
// (SDK error, timeout, or cancel), the session MUST be removed from the
// pool. Otherwise it lingers as a "ghost" in the LRU — subsequent evictions
// pick it, Rust finds no DB binding, and tasks loop in HotSessionLimit
// re-queue until the 5-minute deadline expires.
//
// The fix: turn_auto's finally block closes the session when
// `!reused && !turnSucceeded`. A reused session that fails is NOT closed
// (it stays in the pool for future reuse).
describe('turn_auto ghost session cleanup', () => {
  it('removes newly-created session from pool when sendTurn throws (SDK error)', async () => {
    const pool = new SessionPool(3, fakeFactory({
      promptError: { status: 401, message: 'Unauthorized' },
    }));
    const handler = turnAutoHandlerWithPool(pool);

    await expect(handler({ ...BASE_PARAMS, logical_subagent_id: 'sub-err' }))
      .rejects.toThrow(SidecarError);

    // Ghost cleanup: session removed from pool
    expect(pool.size()).toBe(0);
    expect(pool.getLruCandidate()).toBeUndefined();
  });

  it('removes newly-created session from pool on timeout', async () => {
    const pool = new SessionPool(3, fakeFactory({ hang: true }));
    const handler = turnAutoHandlerWithPool(pool);

    await expect(handler({ ...BASE_PARAMS, logical_subagent_id: 'sub-timeout', timeout_ms: 50 }))
      .rejects.toThrow(SidecarError);

    // Ghost cleanup: timed-out session removed from pool
    expect(pool.size()).toBe(0);
    expect(pool.getLruCandidate()).toBeUndefined();
  });

  it('removes newly-created session from pool on cancel', async () => {
    // Custom SDK: prompt() hangs until abort(), which causes prompt() to
    // reject with an AbortError — mirroring the real SDK's behavior when
    // session.cancel is called mid-turn.
    let rejectPrompt: (err: Error) => void = () => {};
    const hangPromise = new Promise<void>((_, reject) => { rejectPrompt = reject; });
    const cancelFactory: SessionFactory = async (subagent, opts) => {
      const id = `cancel_${subagent}`;
      const sdk: SdkSession = {
        sessionId: id,
        prompt: () => hangPromise,
        getLastAssistantText: () => '',
        getSessionStats: (): SessionStatsLike => ({
          tokens: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
          cost: 0,
        }),
        abort: async () => {
          rejectPrompt(Object.assign(new Error('The user aborted a request'), { name: 'AbortError' }));
        },
        dispose: () => {},
      };
      return new PiSdkSession(sdk, subagent, id, 'test-provider', opts.model);
    };

    const pool = new SessionPool(3, cancelFactory);
    const handler = turnAutoHandlerWithPool(pool);

    // Start the turn — it hangs on prompt()
    const turnPromise = handler({ ...BASE_PARAMS, logical_subagent_id: 'sub-cancel' });

    // Wait for the session to be created and the turn to start
    await new Promise(r => setTimeout(r, 20));

    // Cancel the in-flight turn via the pool's abort mechanism
    await pool.abortSession('sub-cancel');

    // The turn should reject with TURN_CANCELLED
    try {
      await turnPromise;
      throw new Error('should have thrown');
    } catch (e) {
      expect(e).toBeInstanceOf(SidecarError);
      expect((e as SidecarError).code).toBe(ERROR_CODE_TURN_CANCELLED);
    }

    // Ghost cleanup: cancelled session removed from pool
    expect(pool.size()).toBe(0);
    expect(pool.getLruCandidate()).toBeUndefined();
  });

  it('does NOT close a reused session when sendTurn fails', async () => {
    // Stateful SDK: succeeds on the first prompt, throws on the second.
    // The factory is called once (creates the session); the second turn
    // reuses it and the SDK's prompt() throws.
    let promptCount = 0;
    const statefulFactory: SessionFactory = async (subagent, opts) => {
      const id = `stateful_${subagent}`;
      const sdk: SdkSession = {
        sessionId: id,
        prompt: async () => {
          promptCount++;
          if (promptCount > 1) {
            throw Object.assign(new Error('Unauthorized'), { status: 401 });
          }
        },
        getLastAssistantText: () => 'first turn ok',
        getSessionStats: (): SessionStatsLike => ({
          tokens: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, total: 2 },
          cost: 0,
        }),
        abort: async () => {},
        dispose: () => {},
      };
      return new PiSdkSession(sdk, subagent, id, 'test-provider', opts.model);
    };

    const pool = new SessionPool(3, statefulFactory);
    const handler = turnAutoHandlerWithPool(pool);

    // First turn: succeeds, session stays in pool
    const r1 = await handler({ ...BASE_PARAMS, logical_subagent_id: 'sub-reuse' }) as {
      adapter_session_id: string;
    };
    expect(pool.size()).toBe(1);
    const sessionId = r1.adapter_session_id;

    // Second turn: same subagent → reuses session → SDK fails (2nd prompt)
    await expect(handler({ ...BASE_PARAMS, logical_subagent_id: 'sub-reuse', prompt: 'second' }))
      .rejects.toThrow(SidecarError);

    // Reused session is NOT closed — stays in pool for future reuse
    expect(pool.size()).toBe(1);
    expect(pool.get(sessionId)).toBeDefined();
    expect(pool.getLruCandidate()).toBe(sessionId);
  });

  it('maxHot=1: A fails → B can immediately succeed (slot freed by ghost cleanup)', async () => {
    // Factory: first session fails (auth error), second session succeeds.
    let factoryCallCount = 0;
    const crossFactory: SessionFactory = async (subagent, opts) => {
      factoryCallCount++;
      const id = `cross_${factoryCallCount}`;
      const config = factoryCallCount === 1
        ? { promptError: { status: 401, message: 'Unauthorized' } }
        : { assistantText: 'B succeeded' };
      return new PiSdkSession(makeFakeSdk(id, config), subagent, id, 'test-provider', opts.model);
    };

    const pool = new SessionPool(1, crossFactory);
    const handler = turnAutoHandlerWithPool(pool);

    // A's turn: fails with auth error
    await expect(handler({ ...BASE_PARAMS, logical_subagent_id: 'sub-a' }))
      .rejects.toThrow(SidecarError);

    // Ghost cleanup: A's session removed, pool is empty
    expect(pool.size()).toBe(0);

    // B's turn: should succeed — slot was freed by ghost cleanup
    const result = await handler({ ...BASE_PARAMS, logical_subagent_id: 'sub-b' }) as {
      status: string;
      result: { task_summary: string };
    };
    expect(result.status).toBe('completed');
    expect(result.result.task_summary).toBe('B succeeded');
    expect(pool.size()).toBe(1);
  });
});
