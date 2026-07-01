import { describe, it, expect } from 'vitest';
import { SessionPool } from '../src/session_pool.js';
import { SidecarError } from '../src/errors.js';
import { PiSdkSession, type SdkSession, type SessionFactory } from '../src/pi_session.js';

/** Minimal no-op SdkSession stub for pool tests (sendTurn is never called here). */
function stubSdk(id: string): SdkSession {
  return {
    sessionId: id,
    prompt: async () => {},
    subscribe: () => () => {},
    getLastAssistantText: () => '',
    getSessionStats: () => ({
      tokens: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
      cost: 0,
    }),
    abort: async () => {},
    dispose: () => {},
  };
}
function fakeSession(adapterId: string, subagent: string): PiSdkSession {
  return new PiSdkSession(stubSdk(adapterId), subagent, adapterId);
}
/** Factory that yields sessions with the given adapter_session_ids, in order. */
function fakeFactory(...ids: string[]): SessionFactory {
  let i = 0;
  return async (subagent: string) => fakeSession(ids[i++] ?? `fallback_${i}`, subagent);
}

const OPTS = { cwd: '/tmp' };

describe('SessionPool', () => {
  it('ensure creates new session when under limit', async () => {
    const pool = new SessionPool(3);
    const result = await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    expect(result.session.adapter_session_id).toBe('sess-1');
    expect(result.reused).toBe(false);
    expect(pool.size()).toBe(1);
  });

  it('ensure reuses existing session for same subagent', async () => {
    const pool = new SessionPool(3);
    await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    const result = await pool.ensure('sub-a', OPTS, fakeFactory('sess-2'));
    expect(result.session.adapter_session_id).toBe('sess-1');
    expect(result.reused).toBe(true);
    expect(pool.size()).toBe(1);
  });

  it('ensure throws HOT_SESSION_LIMIT_REACHED with candidate when full', async () => {
    const pool = new SessionPool(2);
    await pool.ensure('sub-a', OPTS, fakeFactory('sess-1')); // LRU after next
    await pool.ensure('sub-b', OPTS, fakeFactory('sess-2')); // MRU
    await expect(pool.ensure('sub-c', OPTS, fakeFactory('sess-3'))).rejects.toThrow(SidecarError);
    try {
      await pool.ensure('sub-c', OPTS, fakeFactory('sess-3'));
    } catch (e) {
      expect((e as SidecarError).code).toBe(-32002);
      expect((e as SidecarError).data).toEqual({ candidate: 'sess-1' });
    }
  });

  it('close removes session from pool', async () => {
    const pool = new SessionPool(3);
    await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    pool.close('sess-1');
    expect(pool.size()).toBe(0);
    // Re-ensure creates a new session
    const result = await pool.ensure('sub-a', OPTS, fakeFactory('sess-2'));
    expect(result.session.adapter_session_id).toBe('sess-2');
    expect(result.reused).toBe(false);
  });

  it('LRU order updates on reuse', async () => {
    const pool = new SessionPool(2);
    await pool.ensure('sub-a', OPTS, fakeFactory('sess-1')); // LRU
    await pool.ensure('sub-b', OPTS, fakeFactory('sess-2')); // MRU
    // Reuse sess-1 → it becomes MRU, sess-2 becomes LRU
    await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    await expect(pool.ensure('sub-c', OPTS, fakeFactory('sess-3'))).rejects.toThrow(SidecarError);
    try {
      await pool.ensure('sub-c', OPTS, fakeFactory('sess-3'));
    } catch (e) {
      expect((e as SidecarError).data).toEqual({ candidate: 'sess-2' });
    }
  });

  it('get returns session by adapter_session_id', async () => {
    const pool = new SessionPool(3);
    await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    const session = pool.get('sess-1');
    expect(session).toBeDefined();
    expect(session!.logical_subagent_id).toBe('sub-a');
  });

  it('get returns undefined for unknown session', () => {
    const pool = new SessionPool(3);
    expect(pool.get('unknown')).toBeUndefined();
  });

  it('uses injected factory from constructor when no per-call override', async () => {
    const pool = new SessionPool(3, fakeFactory('from-ctor'));
    const result = await pool.ensure('sub-a', OPTS);
    expect(result.session.adapter_session_id).toBe('from-ctor');
    expect(result.reused).toBe(false);
  });
});
