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

  it('does not exceed maxHot when concurrent misses overlap factory creation', async () => {
    const pool = new SessionPool(1);
    let releaseFactory!: () => void;
    const factoryReady = new Promise<void>((resolve) => {
      releaseFactory = resolve;
    });
    const factory: SessionFactory = async (subagent, opts) => {
      await factoryReady;
      return fakeSession(`sess-${subagent}`, subagent, opts.model);
    };

    const first = pool.ensure('sub-a', OPTS, factory);
    const second = pool.ensure('sub-b', OPTS, factory);
    await expect(second).rejects.toMatchObject({
      code: -32002,
      data: { candidate: null, all_busy: true },
    });
    releaseFactory();
    await expect(first).resolves.toMatchObject({ reused: false });
    expect(pool.size()).toBe(1);
  });

  it('does not create duplicate sessions for a concurrent same-subagent miss', async () => {
    const pool = new SessionPool(2);
    let releaseFactory!: () => void;
    const factoryReady = new Promise<void>((resolve) => {
      releaseFactory = resolve;
    });
    let factoryCalls = 0;
    const factory: SessionFactory = async (subagent, opts) => {
      factoryCalls += 1;
      await factoryReady;
      return fakeSession(`sess-${factoryCalls}`, subagent, opts.model);
    };

    const first = pool.ensure('sub-a', OPTS, factory);
    const second = pool.ensure('sub-a', OPTS, factory);
    await expect(second).rejects.toMatchObject({
      code: -32002,
      data: { candidate: null, all_busy: true },
    });
    releaseFactory();
    await first;
    expect(factoryCalls).toBe(1);
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

  it('rejects a second turn while the same logical session is busy', async () => {
    const pool = new SessionPool(2);
    const first = await pool.ensureForTurn('sub-a', OPTS, fakeFactory('sess-1'));
    expect(first.reused).toBe(false);
    await expect(pool.ensureForTurn('sub-a', OPTS, fakeFactory('sess-2')))
      .rejects.toMatchObject({
        code: -32002,
        data: { candidate: null, all_busy: true },
      });
    pool.endTurn(first.session.adapter_session_id);
    expect(pool.size()).toBe(1);
  });

  it('leases a reused session before the ensureForTurn promise yields', async () => {
    const pool = new SessionPool(1);
    await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
    pool.activate('sess-1');

    // A concurrent prepare_hibernate must observe the lease immediately,
    // even though ensureForTurn returns a Promise for the hit path.
    const turn = pool.ensureForTurn('sub-a', OPTS, fakeFactory('sess-2'));
    expect(pool.reserveForEviction('sess-1')).toBe(false);
    await expect(turn).resolves.toMatchObject({ reused: true });
    pool.endTurn('sess-1');
  });

  it('cancels an admitted turn before the handler starts the SDK call', async () => {
    const pool = new SessionPool(1);
    const { session } = await pool.ensureForTurn('sub-a', OPTS, fakeFactory('sess-1'), 'task-new');

    await expect(pool.abortSession('sub-a', 'task-old')).resolves.toBe(false);
    await expect(pool.abortSession('sub-a', 'task-new')).resolves.toBe(true);
    expect(pool.markTurnStarted(session.adapter_session_id, 'task-new')).toBe(false);
    pool.endTurn(session.adapter_session_id);
  });

  it('cancels a session while its factory is still pending', async () => {
    const pool = new SessionPool(1);
    let releaseFactory!: () => void;
    const factoryReady = new Promise<void>((resolve) => {
      releaseFactory = resolve;
    });
    const factory: SessionFactory = async (subagent, opts) => {
      await factoryReady;
      return fakeSession('cancelled-before-publish', subagent, opts.model);
    };

    const turn = pool.ensureForTurn('sub-a', OPTS, factory, 'task-new');
    await Promise.resolve();
    await expect(pool.abortSession('sub-a', 'task-old')).resolves.toBe(false);
    expect(pool.size()).toBe(0);
    await expect(pool.abortSession('sub-a', 'task-new')).resolves.toBe(true);
    releaseFactory();
    await expect(turn).rejects.toMatchObject({ code: -32013 });
    expect(pool.size()).toBe(0);
  });

  it('ensure throws HOT_SESSION_LIMIT_REACHED with candidate when full', async () => {
    const pool = new SessionPool(2);
    pool.activate((await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'))).session.adapter_session_id); // LRU after next
    pool.activate((await pool.ensure('sub-b', OPTS, fakeFactory('sess-2'))).session.adapter_session_id); // MRU
    try {
      await pool.ensure('sub-c', OPTS, fakeFactory('sess-3'));
      throw new Error('should have thrown');
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
    try {
      await pool.ensure('sub-c', OPTS, fakeFactory('sess-3'));
      throw new Error('should have thrown');
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

  it('blocks reuse after an LRU candidate is reserved for eviction', async () => {
    const pool = new SessionPool(1);
    pool.activate((await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'))).session.adapter_session_id);

    expect(pool.getLruCandidate()).toBe('sess-1');

    // The eviction candidate has been handed to Rust. A concurrent turn for
    // the same logical subagent must not reuse the session while the
    // prepare_hibernate → DB flip → close sequence is in flight.
    await expect(pool.ensure('sub-a', OPTS, fakeFactory('sess-2')))
      .rejects.toMatchObject({
        code: -32002,
        data: { candidate: null, all_busy: true },
      });
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

    it('does not expire a busy pending session and starts orphan TTL after endTurn', async () => {
      const pool = new SessionPool(1);
      await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));
      pool.beginTurn('sess-1');

      // A long-running turn may legitimately exceed the pending TTL. The
      // in-flight session must remain available for its current turn and
      // cannot be reclaimed as an orphan.
      vi.advanceTimersByTime(60_001);
      await expect(pool.ensure('sub-b', OPTS, fakeFactory('sess-2')))
        .rejects.toThrow();
      expect(pool.get('sess-1')).toBeDefined();
      expect(pool.isPending('sess-1')).toBe(true);

      // Once the turn ends, the orphan grace period starts. It is measured
      // from endTurn rather than from session creation so Rust has a full
      // window to commit the binding and activate the session.
      pool.endTurn('sess-1');
      vi.advanceTimersByTime(59_999);
      await expect(pool.ensure('sub-b', OPTS, fakeFactory('sess-2')))
        .rejects.toThrow();
      expect(pool.get('sess-1')).toBeDefined();

      vi.advanceTimersByTime(2);
      const result = await pool.ensure('sub-b', OPTS, fakeFactory('sess-2'));
      expect(result.session.adapter_session_id).toBe('sess-2');
      expect(pool.get('sess-1')).toBeUndefined();
    });

    it('cancels pending cleanup when activation wins the race', async () => {
      const pool = new SessionPool(1);
      const { session } = await pool.ensure('sub-a', OPTS, fakeFactory('sess-1'));

      await pool.abortSession('sub-a');
      pool.activate(session.adapter_session_id);
      vi.advanceTimersByTime(5_001);

      expect(pool.get(session.adapter_session_id)).toBeDefined();
      expect(pool.isPending(session.adapter_session_id)).toBe(false);
    });
  });
});
