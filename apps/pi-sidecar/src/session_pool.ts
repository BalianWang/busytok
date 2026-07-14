import {
  PiSdkSession,
  defaultSessionFactory,
  type CreateSessionOpts,
  type SessionFactory,
} from './pi_session.js';
import { SidecarError } from './errors.js';
import { logger } from './logger.js';

/**
 * Sidecar-side hot session pool with LRU tracking (spec §5.2).
 *
 * Tracks adapter_session_id ↔ logical_subagent_id mappings. When the pool
 * is full and a new session is requested, throws HOT_SESSION_LIMIT_REACHED
 * (-32002) with `data.candidate` naming the LRU session — busytok-service
 * then drives eviction via prepare_hibernate + close.
 *
 * **Two-phase lifecycle (P0 fix):** newly-created sessions start in the
 * `pending` state — they are NOT in the LRU and cannot be evicted. This
 * closes the timing window between `turn_auto` returning success (which
 * makes the session idle/evictable) and Rust committing the DB hot binding.
 * Without this lifecycle, a concurrent delegate could evict a session whose
 * binding hasn't been committed yet — creating a "ghost" (sidecar session
 * exists, DB binding doesn't) that corrupts subsequent evictions.
 *
 * The flow:
 * 1. `ensure()` miss → factory creates session → added to `pendingSessions`
 *    (NOT `lru`). Session is usable for the current turn but not evictable.
 * 2. `turn_auto` runs. On failure (new session), the session is closed
 *    (removed entirely). On success, the session stays `pending`.
 * 3. Rust commits the DB hot binding.
 * 4. Rust calls `session.activate(adapter_session_id)` → session moves
 *    from `pendingSessions` to `lru` (becomes evictable).
 * 5. If the DB commit fails, Rust calls `session.close` to clean up the
 *    orphaned pending session.
 *
 * **In-use tracking (Bug 1 fix):** sessions that are currently running a
 * turn (`beginTurn` called, `endTurn` not yet called) are excluded from
 * LRU eviction candidates. This prevents evicting a session whose
 * `turn_auto` is still in-flight. When the pool is full and ALL sessions
 * are in-use or pending, the error includes `data.all_busy = true` and
 * `data.candidate = null` so the executor knows NOT to attempt eviction.
 *
 * Sessions are real `PiSdkSession` wrappers around the Pi SDK's
 * `AgentSession`. The factory is injectable so tests can supply fakes
 * without `vi.mock` (Phase 3 Task 6).
 *
 * LRU is maintained as an ordered array: index 0 = MRU, last = LRU.
 * On reuse, the session moves to the front. On close, it is removed.
 */
export class SessionPool {
  private readonly maxHot: number;
  private readonly factory: SessionFactory;
  private readonly sessions = new Map<string, PiSdkSession>();       // adapter_session_id → session
  private readonly subagentMap = new Map<string, string>();          // logical_subagent_id → adapter_session_id
  private readonly lru: string[] = [];                               // adapter_session_ids, MRU first (active only)
  private readonly busySessions = new Set<string>();                 // adapter_session_ids with in-flight turns
  private readonly pendingSessions = new Set<string>();              // adapter_session_ids not yet activated

  constructor(maxHot: number, factory: SessionFactory = defaultSessionFactory) {
    if (maxHot < 1) throw new Error(`maxHot must be >= 1, got ${maxHot}`);
    this.maxHot = maxHot;
    this.factory = factory;
  }

  /**
   * Ensure a hot session exists for `logical_subagent_id`.
   * - Hit (same model): bump LRU, return `{ session, reused: true }`.
   * - Hit (model mismatch): forced cold miss — evict the old session and
   *   fall through to the miss path so a fresh session is created with the
   *   new model. This is what makes a task-level `model_override` take
   *   effect on a hot session (P1-1): without it, the old session bound to
   *   the previous model would be silently reused.
   * - Miss + capacity: call the factory, add to pool, return `{ reused: false }`.
   * - Miss + full: throw HOT_SESSION_LIMIT_REACHED with `data.candidate`.
   *
   * `createOverride` (optional) lets a caller swap the factory for a single
   * call — used by the mock `turn_auto` path to inject mock sessions without
   * touching the pool's real factory.
   */
  async ensure(
    logical_subagent_id: string,
    opts: CreateSessionOpts,
    createOverride?: SessionFactory,
  ): Promise<{ session: PiSdkSession; reused: boolean }> {
    // 1. Hit — subagent already has a hot session
    const existing = this.subagentMap.get(logical_subagent_id);
    if (existing !== undefined) {
      const session = this.sessions.get(existing);
      if (session) {
        // P1-1: forced cold miss on model mismatch. A task-level
        // `model_override` changes the effective model; the old session
        // (bound to a different model) MUST be evicted and a fresh one
        // created so the override actually takes effect. The Rust side
        // already computes effective_model_id = model_override.unwrap_or(bound).
        if (session.resolvedModel !== opts.model) {
          // Evict the old session (frees the slot + disposes the SDK
          // session), then fall through to the miss path below to create
          // a fresh session bound to opts.model.
          logger.debug('session_pool.model_mismatch_evict', {
            logical_subagent_id,
            adapter_session_id: existing,
            old_model: session.resolvedModel,
            new_model: opts.model,
          });
          this.close(existing);
        } else {
          this.touch(existing);
          return { session, reused: true };
        }
      } else {
        // Defensive: stale subagent mapping without a session entry.
        this.subagentMap.delete(logical_subagent_id);
      }
    }
    // 2. Miss + full — throw HOT_SESSION_LIMIT_REACHED with `data.candidate`.
    //    Bug 1 fix: skip in-use sessions (currently running a turn) —
    //    evicting them would corrupt the in-flight task AND fail on the
    //    Rust side (no DB binding yet). If all sessions are in-use,
    //    `data.candidate = null` and `data.all_busy = true` so the executor
    //    knows NOT to attempt eviction.
    if (this.sessions.size >= this.maxHot) {
      const candidate = this.getLruCandidate();
      if (candidate === undefined) {
        throw new SidecarError('hot session limit reached — all sessions busy', -32002, {
          candidate: null,
          all_busy: true,
        });
      }
      throw new SidecarError('hot session limit reached', -32002, { candidate });
    }
    // 3. Miss + capacity — create a new session via the factory.
    // The session starts in `pending` state (NOT in LRU). It will be
    // activated (moved to LRU) by Rust calling `session.activate` after
    // the DB hot binding is committed. This closes the timing window
    // where a successful turn_auto makes the session evictable before
    // Rust has committed the binding.
    const factory = createOverride ?? this.factory;
    const session = await factory(logical_subagent_id, opts);
    const adapter_session_id = session.adapter_session_id;
    this.sessions.set(adapter_session_id, session);
    this.subagentMap.set(logical_subagent_id, adapter_session_id);
    this.pendingSessions.add(adapter_session_id);
    return { session, reused: false };
  }

  /** Get a session by adapter_session_id. */
  get(adapter_session_id: string): PiSdkSession | undefined {
    return this.sessions.get(adapter_session_id);
  }

  /**
   * Abort an in-flight turn for `logical_subagent_id`. Looks up the hot
   * session by subagent id and calls `session.abort()`, which aborts the
   * underlying SDK HTTP request to the LLM provider — stopping token
   * generation. The session stays in the pool (not closed) and can be
   * reused for subsequent turns.
   *
   * Called by the `session.cancel` RPC handler. Returns `true` if a hot
   * session was found and abort was called, `false` if no hot session
   * exists for the subagent (the turn may have already completed or the
   * subagent was never seen by this sidecar).
   */
  async abortSession(logical_subagent_id: string): Promise<boolean> {
    const adapter_session_id = this.subagentMap.get(logical_subagent_id);
    if (adapter_session_id === undefined) return false;
    const session = this.sessions.get(adapter_session_id);
    if (!session) return false;
    await session.abort();
    return true;
  }

  /** Close (remove) a session from the pool. Disposes the SDK session. */
  close(adapter_session_id: string): void {
    const session = this.sessions.get(adapter_session_id);
    if (!session) return;
    this.sessions.delete(adapter_session_id);
    this.subagentMap.delete(session.logical_subagent_id);
    const idx = this.lru.indexOf(adapter_session_id);
    if (idx >= 0) this.lru.splice(idx, 1);
    this.busySessions.delete(adapter_session_id);
    this.pendingSessions.delete(adapter_session_id);
    // Dispose the underlying SDK session (fire-and-forget; dispose() is sync).
    void session.close();
  }

  /**
   * Activate a session — move it from `pending` to the LRU (evictable).
   *
   * Called by Rust via the `session.activate` RPC AFTER the DB hot binding
   * is committed. Until activate is called, the session is in the pool
   * (usable for reuse by the same subagent) but NOT in the LRU (cannot be
   * selected as an eviction candidate). This closes the timing window
   * between `turn_auto` success and DB binding commit.
   *
   * Idempotent: if the session is already active (in LRU), this is a no-op.
   * If the session doesn't exist (already closed or never created), it's
   * also a no-op — the caller (Rust) treats activate failure as best-effort.
   */
  activate(adapter_session_id: string): void {
    if (!this.pendingSessions.has(adapter_session_id)) {
      // Already active or not in pool — idempotent no-op.
      return;
    }
    this.pendingSessions.delete(adapter_session_id);
    this.lru.unshift(adapter_session_id); // MRU at front
    logger.debug('session_pool.activated', { adapter_session_id });
  }

  /** Whether a session is in the pending state (not yet activated). */
  isPending(adapter_session_id: string): boolean {
    return this.pendingSessions.has(adapter_session_id);
  }

  /**
   * Mark a session as "in-use" (running a turn). In-use sessions are
   * excluded from LRU eviction candidates — evicting them would corrupt
   * the in-flight turn and trigger a spurious "no hot binding" error on
   * the Rust side (the binding is only persisted AFTER `turn_auto`
   * returns). Must be paired with `endTurn` in a try/finally block.
   */
  beginTurn(adapter_session_id: string): void {
    this.busySessions.add(adapter_session_id);
  }

  /**
   * Mark a session as idle (turn completed). Paired with `beginTurn`.
   * Safe to call on a session that was already removed via `close()` —
   * the busy flag is cleaned up on close as well.
   */
  endTurn(adapter_session_id: string): void {
    this.busySessions.delete(adapter_session_id);
  }

  /** Current number of hot sessions. */
  size(): number {
    return this.sessions.size;
  }

  /** All sessions as an array (for prepare_hibernate all). */
  toArray(): PiSdkSession[] {
    return Array.from(this.sessions.values());
  }

  /** Move a session_id to the MRU position (front of lru array). */
  private touch(adapter_session_id: string): void {
    const idx = this.lru.indexOf(adapter_session_id);
    if (idx > 0) {
      this.lru.splice(idx, 1);
      this.lru.unshift(adapter_session_id);
    }
    const session = this.sessions.get(adapter_session_id);
    if (session) {
      session.last_used_at_ms = Date.now();
    }
  }

  /**
   * Get the LRU evictable candidate. Skips sessions currently running a
   * turn (`beginTurn` without `endTurn`). Returns `undefined` when no
   * evictable session exists (all sessions are in-use or pool is empty).
   *
   * Concurrent eviction race is handled at the DB layer: if two delegates
   * both get the same candidate, the second one's `evict_session` detects
   * `is_hot=0` (already flipped by the first) and returns `AlreadyEvicted`.
   */
  getLruCandidate(): string | undefined {
    for (let i = this.lru.length - 1; i >= 0; i--) {
      const id = this.lru[i]!;
      if (this.busySessions.has(id)) continue;
      return id;
    }
    return undefined;
  }
}
