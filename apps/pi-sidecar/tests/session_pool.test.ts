import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { SessionPool } from '../src/session_pool.js';
import { SidecarError } from '../src/errors.js';
import { PiSdkSession, type SdkSession, type SessionFactory } from '../src/pi_session.js';

/** Minimal no-op SdkSession stub for pool tests (sendTurn is never called here). */
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
/** Factory that yields sessions with the given adapter_session_ids, in order.
 * Threads `opts.model` into each session's `resolvedModel` so the pool's
 * model-mismatch cold-miss (P1-1) is exercisable. */
function fakeFactory(...ids: string[]): SessionFactory {
  let i = 0;
  return async (subagent: string, opts) => fakeSession(ids[i++] ?? `fallback_${i}`, subagent, opts.model);
}

const OPTS = { cwd: '/tmp', model: 'test-model' };

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
    pool.activate((await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'))).session.adapter_session_id); // LRU after next
    pool.activate((await pool.ensure('sub-b', OPTS, fakeFactory('sess-2'))).session.adapter_session_id); // MRU
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
    pool.activate((await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'))).session.adapter_session_id); // LRU
    pool.activate((await pool.ensure('sub-b', OPTS, fakeFactory('sess-2'))).session.adapter_session_id); // MRU
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

  it('ensure evicts + recreates session when model changes (model_override on hot session)', async () => {
    // P1-1 regression: a task-level `model_override` changes the effective
    // model. The hot pool MUST evict the old session (bound to the previous
    // model) and create a fresh one — NOT silently reuse the old session.
    const pool = new SessionPool(3);
    await pool.ensure('sub-a', { ...OPTS, model: 'model-a' }, fakeFactory('sess-1'));
    expect(pool.size()).toBe(1);
    expect(pool.get('sess-1')).toBeDefined();

    const result = await pool.ensure(
      'sub-a',
      { ...OPTS, model: 'model-b' },
      fakeFactory('sess-2'),
    );

    // A new session was created (not reused).
    expect(result.reused).toBe(false);
    expect(result.session.adapter_session_id).toBe('sess-2');
    expect(result.session.resolvedModel).toBe('model-b');
    // Pool size stays 1: old session evicted, new one created.
    expect(pool.size()).toBe(1);
    // The old session is gone.
    expect(pool.get('sess-1')).toBeUndefined();
    // The new session is present.
    expect(pool.get('sess-2')).toBeDefined();
  });

  it('ensure reuses session when model is unchanged (same model string)', async () => {
    // P1-1 sanity: when the model matches, the hot session is reused as before.
    const pool = new SessionPool(3);
    await pool.ensure('sub-a', { ...OPTS, model: 'same-model' }, fakeFactory('sess-1'));
    const result = await pool.ensure(
      'sub-a',
      { ...OPTS, model: 'same-model' },
      fakeFactory('sess-2'),
    );
    expect(result.reused).toBe(true);
    expect(result.session.adapter_session_id).toBe('sess-1');
    expect(pool.size()).toBe(1);
  });

  it('ensure cold-miss on model mismatch frees a slot when pool is full', async () => {
    // P1-1: when the pool is full and a model override arrives for an
    // existing subagent, the old session is evicted (freeing a slot) and a
    // new one created — HOT_SESSION_LIMIT_REACHED is NOT thrown because the
    // eviction made room.
    const pool = new SessionPool(1);
    await pool.ensure('sub-a', { ...OPTS, model: 'model-a' }, fakeFactory('sess-1'));
    expect(pool.size()).toBe(1);
    const result = await pool.ensure(
      'sub-a',
      { ...OPTS, model: 'model-b' },
      fakeFactory('sess-2'),
    );
    expect(result.reused).toBe(false);
    expect(result.session.adapter_session_id).toBe('sess-2');
    expect(pool.size()).toBe(1);
    expect(pool.get('sess-1')).toBeUndefined();
  });

  // --- Bug 1 fix: in-use (busy) session tracking ---

  it('getLruCandidate skips in-use sessions', async () => {
    // A session running a turn (beginTurn without endTurn) must NOT be
    // selected as an eviction candidate — evicting it would corrupt the
    // in-flight task and fail on the Rust side (no DB binding yet).
    const pool = new SessionPool(2);
    pool.activate((await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'))).session.adapter_session_id); // LRU
    pool.activate((await pool.ensure('sub-b', OPTS, fakeFactory('sess-2'))).session.adapter_session_id); // MRU
    pool.beginTurn('sess-1');
    // sess-1 is busy → getLruCandidate skips it, returns sess-2 instead.
    expect(pool.getLruCandidate()).toBe('sess-2');
  });

  it('getLruCandidate returns undefined when all sessions are in-use', async () => {
    // When every session is busy, there is no safe eviction candidate.
    const pool = new SessionPool(2);
    pool.activate((await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'))).session.adapter_session_id);
    pool.activate((await pool.ensure('sub-b', OPTS, fakeFactory('sess-2'))).session.adapter_session_id);
    pool.beginTurn('sess-1');
    pool.beginTurn('sess-2');
    expect(pool.getLruCandidate()).toBeUndefined();
  });

  it('endTurn restores session evictability', async () => {
    // After endTurn, the session is again a valid LRU candidate.
    const pool = new SessionPool(1);
    pool.activate((await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'))).session.adapter_session_id);
    pool.beginTurn('sess-1');
    expect(pool.getLruCandidate()).toBeUndefined();
    pool.endTurn('sess-1');
    expect(pool.getLruCandidate()).toBe('sess-1');
  });

  it('ensure throws all_busy when pool is full and all sessions are in-use', async () => {
    // Bug 1 core scenario: pool full + all sessions busy → the sidecar
    // returns `data.candidate = null` + `data.all_busy = true` so the
    // executor knows NOT to attempt eviction.
    const pool = new SessionPool(1);
    pool.activate((await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'))).session.adapter_session_id);
    pool.beginTurn('sess-1');
    try {
      await pool.ensure('sub-b', OPTS, fakeFactory('sess-2'));
      throw new Error('should have thrown');
    } catch (e) {
      expect(e).toBeInstanceOf(SidecarError);
      expect((e as SidecarError).code).toBe(-32002);
      expect((e as SidecarError).data).toEqual({
        candidate: null,
        all_busy: true,
      });
    }
  });

  it('ensure names non-busy candidate when pool is full but some sessions are idle', async () => {
    // Mixed: pool full, one busy + one idle → the idle session is the
    // eviction candidate (not the busy one).
    const pool = new SessionPool(2);
    pool.activate((await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'))).session.adapter_session_id); // LRU, busy
    pool.activate((await pool.ensure('sub-b', OPTS, fakeFactory('sess-2'))).session.adapter_session_id); // MRU, idle
    pool.beginTurn('sess-1');
    try {
      await pool.ensure('sub-c', OPTS, fakeFactory('sess-3'));
      throw new Error('should have thrown');
    } catch (e) {
      expect(e).toBeInstanceOf(SidecarError);
      expect((e as SidecarError).code).toBe(-32002);
      // candidate is sess-2 (idle), NOT sess-1 (busy).
      expect((e as SidecarError).data).toEqual({ candidate: 'sess-2' });
    }
  });

  it('close cleans up busy state', async () => {
    // close() must remove the session from busySessions — otherwise a
    // stale busy flag could block future evictions after the session is gone.
    const pool = new SessionPool(1);
    await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    pool.beginTurn('sess-1');
    pool.close('sess-1');
    expect(pool.size()).toBe(0);
    // After close, a new session can be created (no stale busy block).
    const result = await pool.ensure('sub-a', OPTS, fakeFactory('sess-2'));
    expect(result.session.adapter_session_id).toBe('sess-2');
  });

  it('endTurn is safe on a session that was already closed', async () => {
    // endTurn must be a no-op (not throw) if the session was removed via
    // close before endTurn was called — mirrors the try/finally contract
    // in turn_auto where close can happen between beginTurn and endTurn.
    const pool = new SessionPool(1);
    await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    pool.beginTurn('sess-1');
    pool.close('sess-1');
    expect(() => pool.endTurn('sess-1')).not.toThrow();
  });

  it('activate throws SESSION_NOT_FOUND for unknown session', async () => {
    // activate on a session that doesn't exist (closed, never created, or
    // lost to sidecar restart) must throw SESSION_NOT_FOUND — returning
    // success would create a false is_hot=1 ghost binding in the DB.
    const pool = new SessionPool(3);
    await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));

    // Unknown session id — should throw, not return void.
    expect(() => pool.activate('nonexistent-session')).toThrow(SidecarError);
    try {
      pool.activate('nonexistent-session');
    } catch (e) {
      expect((e as SidecarError).code).toBe(-32001); // SESSION_NOT_FOUND
    }
  });

  it('activate is idempotent for already-active session', async () => {
    // Activating a session that's already in the LRU (already active)
    // must be a no-op (not throw).
    const pool = new SessionPool(3);
    const { session } = await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    pool.activate(session.adapter_session_id);
    // Second activate — should not throw.
    expect(() => pool.activate(session.adapter_session_id)).not.toThrow();
  });

  it('activate on closed session throws SESSION_NOT_FOUND', async () => {
    // After close, the session is no longer in the pool. activate should
    // throw SESSION_NOT_FOUND so Rust knows to roll back the binding.
    const pool = new SessionPool(3);
    const { session } = await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    pool.activate(session.adapter_session_id);
    pool.close(session.adapter_session_id);
    expect(() => pool.activate(session.adapter_session_id)).toThrow(SidecarError);
    try {
      pool.activate(session.adapter_session_id);
    } catch (e) {
      expect((e as SidecarError).code).toBe(-32001);
    }
  });

  describe('pending TTL', () => {
    beforeEach(() => {
      vi.useFakeTimers();
    });

    afterEach(() => {
      vi.useRealTimers();
    });

    it('evictExpiredPending closes pending sessions older than TTL', async () => {
      // Safety net: if Rust crashes between turn_auto and activate/close,
      // the orphaned pending session is closed after the TTL to free the
      // capacity slot. Without this, the pending session would occupy a
      // slot indefinitely (pending sessions are not in LRU, not evictable).
      const pool = new SessionPool(1);
      // Create a pending session (don't activate it).
      await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
      expect(pool.size()).toBe(1);
      expect(pool.isPending('sess-1')).toBe(true);

      // Advance time past the TTL (60s).
      vi.advanceTimersByTime(60_001);

      // Trigger eviction by calling ensure() for a different subagent.
      // evictExpiredPending runs at the top of ensure() and closes the
      // expired pending session, freeing the slot for the new subagent.
      const result = await pool.ensure('sub-b', OPTS, fakeFactory('sess-2'));
      expect(result.session.adapter_session_id).toBe('sess-2');
      expect(result.reused).toBe(false);
      expect(pool.size()).toBe(1);
      expect(pool.isPending('sess-2')).toBe(true);
    });

    it('pending sessions under TTL are not evicted', async () => {
      const pool = new SessionPool(1);
      await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));

      // Advance time but stay under the TTL.
      vi.advanceTimersByTime(59_999);

      // ensure() for a different subagent should still see the pool as
      // full (pending session not expired yet).
      await expect(pool.ensure('sub-b', OPTS, fakeFactory('sess-2')))
        .rejects.toThrow();
      // The pending session is still there.
      expect(pool.isPending('sess-1')).toBe(true);
    });
  });
});
