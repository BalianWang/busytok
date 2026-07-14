import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { initializeHandler } from '../src/handlers/initialize.js';
import { healthHandlerWithPool } from '../src/handlers/health.js';
import { shutdownHandler } from '../src/handlers/shutdown.js';
import { turnAutoHandlerWithPool } from '../src/handlers/turn_auto.js';
import { prepareHibernateHandlerWithPool } from '../src/handlers/prepare_hibernate.js';
import { closeHandlerWithPool } from '../src/handlers/close.js';
import { cancelHandlerWithPool } from '../src/handlers/cancel.js';
import { SessionPool } from '../src/session_pool.js';
import { PiSdkSession, type SdkSession, type SessionFactory } from '../src/pi_session.js';
import type { HandlerContext } from '../src/rpc.js';

const noopCtx: HandlerContext = { stop: () => {} };

function stubSdk(id: string): SdkSession {
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
function fakeSession(adapterId: string, subagent: string, model = 'test-model'): PiSdkSession {
  return new PiSdkSession(stubSdk(adapterId), subagent, adapterId, 'test-provider', model);
}
function fakeFactory(...ids: string[]): SessionFactory {
  let i = 0;
  return async (subagent: string, opts) => fakeSession(ids[i++] ?? `fallback_${i}`, subagent, opts.model);
}

const OPTS = { cwd: '/tmp', model: 'test-model' };

describe('initialize handler', () => {
  it('returns protocol version on match', async () => {
    const result = await initializeHandler({ protocol_version: 1 }, noopCtx) as {
      protocol_version: number; sidecar_version: string;
    };
    expect(result.protocol_version).toBe(1);
    expect(result.sidecar_version).toBe('0.1.0');
  });

  it('throws PROTOCOL_MISMATCH on version mismatch', async () => {
    await expect(initializeHandler({ protocol_version: 99 }, noopCtx)).rejects.toThrow();
    try {
      await initializeHandler({ protocol_version: 99 }, noopCtx);
    } catch (e) {
      expect((e as { code: number }).code).toBe(-32008);
    }
  });
});

describe('health handler', () => {
  it('returns healthy status with real session count', async () => {
    const pool = new SessionPool(3);
    const handler = healthHandlerWithPool(pool);
    const result = await handler({}, noopCtx) as {
      status: string; sessions: number; rss_mb: number;
    };
    expect(result.status).toBe('healthy');
    expect(result.sessions).toBe(0);
    expect(result.rss_mb).toBeGreaterThan(0);

    // Ensure a session and confirm health reflects the new count.
    await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    const result2 = await handler({}, noopCtx) as { sessions: number };
    expect(result2.sessions).toBe(1);
  });
});

describe('shutdown handler', () => {
  it('calls ctx.stop() and returns ok', async () => {
    let stopped = false;
    const ctx: HandlerContext = { stop: () => { stopped = true; } };
    const result = await shutdownHandler({}, ctx) as { ok: boolean };
    expect(result.ok).toBe(true);
    expect(stopped).toBe(true);
  });
});

describe('turn_auto handler (mock path)', () => {
  const PREV = process.env.BUSYTOK_USE_MOCK_SIDECAR;
  beforeAll(() => { process.env.BUSYTOK_USE_MOCK_SIDECAR = '1'; });
  afterAll(() => {
    if (PREV === undefined) delete process.env.BUSYTOK_USE_MOCK_SIDECAR;
    else process.env.BUSYTOK_USE_MOCK_SIDECAR = PREV;
  });

  it('returns completed result with usage', async () => {
    const pool = new SessionPool(3);
    const handler = turnAutoHandlerWithPool(pool);
    const result = await handler(
      { logical_subagent_id: 'sa_1', prompt: 'do the thing', cwd: '/tmp', profile: 'pi/default', model: 'test-model' },
      noopCtx,
    ) as {
      adapter_session_id: string; status: string;
      result: { task_summary: string };
      usage: { input_tokens: number; output_tokens: number };
    };
    expect(result.status).toBe('completed');
    expect(result.adapter_session_id).toMatch(/^pi_sess_mock_/);
    expect(result.usage.input_tokens).toBe('do the thing'.length);
    expect(result.usage.output_tokens).toBe(50);
  });

  it('throws on missing required fields', async () => {
    const pool = new SessionPool(3);
    const handler = turnAutoHandlerWithPool(pool);
    await expect(handler({ cwd: '/tmp' }, noopCtx)).rejects.toThrow();
    try {
      await handler({ cwd: '/tmp' }, noopCtx);
    } catch (e) {
      expect((e as { code: number }).code).toBe(-32602);
    }
  });
});

describe('prepare_hibernate handler', () => {
  it('compacts a single session by adapter_session_id', async () => {
    const pool = new SessionPool(3);
    const { session } = await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    pool.activate(session.adapter_session_id);
    const handler = prepareHibernateHandlerWithPool(pool);
    const result = await handler({ adapter_session_id: 'sess-1' }, noopCtx) as {
      memory_delta: { hot_summary?: string } | null;
      stats: Record<string, unknown>;
    };
    expect(result.memory_delta?.hot_summary).toContain('sess-1');
    expect(result.stats.subagent_id).toBe('sub-a');
  });

  it('compacts all sessions when all:true', async () => {
    const pool = new SessionPool(3);
    await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    await pool.ensure('sub-b', OPTS, fakeFactory('sess-2'));
    const handler = prepareHibernateHandlerWithPool(pool);
    const result = await handler({ all: true }, noopCtx) as {
      stats: { sessions_compacted: number };
      sessions: { adapter_session_id: string; logical_subagent_id: string }[];
    };
    expect(result.stats.sessions_compacted).toBe(2);
    expect(result.sessions.map((s) => s.adapter_session_id).sort()).toEqual(['sess-1', 'sess-2']);
  });

  it('rejects a busy session instead of preparing it for eviction', async () => {
    const pool = new SessionPool(1);
    const { session } = await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    pool.activate(session.adapter_session_id);
    pool.beginTurn(session.adapter_session_id);
    const handler = prepareHibernateHandlerWithPool(pool);

    await expect(handler({ adapter_session_id: 'sess-1' }, noopCtx)).rejects.toMatchObject({
      code: -32002,
      data: { candidate: null, all_busy: true },
    });
  });

  it('throws -32602 when neither all nor adapter_session_id provided', async () => {
    const pool = new SessionPool(3);
    const handler = prepareHibernateHandlerWithPool(pool);
    await expect(handler({}, noopCtx)).rejects.toThrow();
    try {
      await handler({}, noopCtx);
    } catch (e) {
      expect((e as { code: number }).code).toBe(-32602);
    }
  });

  it('throws -32001 when session not found', async () => {
    const pool = new SessionPool(3);
    const handler = prepareHibernateHandlerWithPool(pool);
    try {
      await handler({ adapter_session_id: 'nope' }, noopCtx);
    } catch (e) {
      expect((e as { code: number }).code).toBe(-32001);
    }
  });
});

describe('close handler', () => {
  it('closes an existing session and returns ok', async () => {
    const pool = new SessionPool(3);
    await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    const handler = closeHandlerWithPool(pool);
    const result = await handler({ adapter_session_id: 'sess-1' }, noopCtx) as { ok: boolean };
    expect(result.ok).toBe(true);
    expect(pool.size()).toBe(0);
  });

  it('throws -32602 when adapter_session_id missing', async () => {
    const pool = new SessionPool(3);
    const handler = closeHandlerWithPool(pool);
    try {
      await handler({}, noopCtx);
    } catch (e) {
      expect((e as { code: number }).code).toBe(-32602);
    }
  });

  it('throws -32001 when session not found', async () => {
    const pool = new SessionPool(3);
    const handler = closeHandlerWithPool(pool);
    try {
      await handler({ adapter_session_id: 'nope' }, noopCtx);
    } catch (e) {
      expect((e as { code: number }).code).toBe(-32001);
    }
  });
});

describe('cancel handler', () => {
  it('aborts an existing session and returns cancelled: true', async () => {
    const pool = new SessionPool(3);
    let abortCalled = false;
    let rejectPrompt: ((reason?: unknown) => void) | undefined;
    const spySdk: SdkSession = {
      sessionId: 'sess-cancel',
      prompt: () =>
        new Promise<void>((_, reject) => {
          rejectPrompt = reject;
        }),
      getLastAssistantText: () => '',
      getSessionStats: () => ({
        tokens: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
        cost: 0,
      }),
      abort: async () => {
        abortCalled = true;
        rejectPrompt?.(new Error('aborted'));
      },
      dispose: () => {},
    };
    const factory: SessionFactory = async (subagent: string) =>
      new PiSdkSession(spySdk, subagent, 'sess-cancel', 'test-provider', 'test-model');
    await pool.ensure('sub-cancel', OPTS, factory);
    const session = pool.get('sess-cancel');
    expect(session).toBeDefined();
    const turn = session!.sendTurn('cancel me', { task_id: 'task-cancel' }).catch(() => {});
    pool.beginTurn('sess-cancel');
    const handler = cancelHandlerWithPool(pool);
    const result = await handler(
      { logical_subagent_id: 'sub-cancel', task_id: 'task-cancel' },
      noopCtx,
    ) as { cancelled: boolean };
    await turn;
    expect(result.cancelled).toBe(true);
    expect(abortCalled).toBe(true);
    // Session stays in the pool (not closed) — it can be reused.
    expect(pool.size()).toBe(1);
  });

  it('returns cancelled: false when no session exists for the subagent', async () => {
    const pool = new SessionPool(3);
    const handler = cancelHandlerWithPool(pool);
    const result = await handler({ logical_subagent_id: 'nonexistent' }, noopCtx) as { cancelled: boolean };
    expect(result.cancelled).toBe(false);
  });

  it('throws -32602 on missing logical_subagent_id', async () => {
    const pool = new SessionPool(3);
    const handler = cancelHandlerWithPool(pool);
    await expect(handler({}, noopCtx)).rejects.toThrow();
    try {
      await handler({}, noopCtx);
    } catch (e) {
      expect((e as { code: number }).code).toBe(-32602);
    }
  });
});
