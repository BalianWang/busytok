import {
  PiSdkSession,
  defaultSessionFactory,
  type CreateSessionOpts,
  type SessionFactory,
} from './pi_session.js';
import { SidecarError } from './errors.js';
import { logger } from './logger.js';

/**
 * TTL for pending sessions (milliseconds). If a session stays pending longer
 * than this, it's considered orphaned (Rust likely crashed between turn_auto
 * and activate/close) and is closed to free the capacity slot. In-flight
 * turns are excluded; cancellation has a shorter, explicit grace below.
 */
const PENDING_TTL_MS = 60_000;

/**
 * Bounded cleanup grace after cancellation. Some SDKs acknowledge abort but
 * never settle the prompt promise, so relying on turn_auto's finally block
 * alone can leave a session permanently quarantined. The fallback closes both
 * pending and activated sessions; no replacement turn is admitted while the
 * cancellation is pending.
 */
const PENDING_CANCEL_GRACE_MS = 5_000;

/**
 * A candidate reservation normally lasts until Rust completes
 * prepare_hibernate → DB flip → close. If Rust crashes or loses the RPC
 * response, release it lazily on the next pool operation so one abandoned
 * eviction cannot block the pool forever.
 */
const EVICTION_RESERVATION_TTL_MS = 120_000;

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
  private readonly turnLeases = new Map<string, {
    taskId?: string;
    started: boolean;
    cancelled: boolean;
  }>();
  private readonly pendingSessions = new Set<string>();              // adapter_session_ids not yet activated
  // Reservations are made before awaiting the async factory so concurrent
  // misses cannot oversubscribe maxHot or create duplicate sessions for one
  // logical identity.
  private readonly creatingSubagents = new Map<string, string | undefined>();
  private readonly cancelledCreations = new Set<string>();
  private readonly pendingSince = new Map<string, number>();        // adapter_session_id → orphan-watch timestamp (ms)
  private readonly pendingCancelCleanup = new Map<string, ReturnType<typeof setTimeout>>(); // session → cancel fallback
  private readonly evictingSessions = new Map<string, number>();     // adapter_session_id → reservation timestamp

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
    creationTaskId?: string,
    acquireTurn = false,
  ): Promise<{ session: PiSdkSession; reused: boolean }> {
    this.releaseExpiredEvictions();
    // 0. Safety net: close expired pending sessions. If Rust crashed
    //    between turn_auto and activate/close, the pending session would
    //    linger forever, occupying a capacity slot. A pending session
    //    older than PENDING_TTL_MS is stale — close it and free the slot
    //    before the capacity check below.
    this.evictExpiredPending();

    // 1. Hit — subagent already has a hot session
    const existing = this.subagentMap.get(logical_subagent_id);
    if (existing !== undefined) {
      const session = this.sessions.get(existing);
      if (session) {
        if (this.evictingSessions.has(existing) || session.isCancellationPending()) {
          throw new SidecarError(
            'hot session eviction in progress — all sessions are temporarily unavailable',
            -32002,
            { candidate: null, all_busy: true },
          );
        }
        if (this.busySessions.has(existing)) {
          throw new SidecarError(
            'logical subagent already has a turn in progress',
            -32002,
            { candidate: null, all_busy: true },
          );
        }
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
          if (acquireTurn) this.acquireTurn(existing, creationTaskId);
          return { session, reused: true };
        }
      } else {
        // Defensive: stale subagent mapping without a session entry.
        this.subagentMap.delete(logical_subagent_id);
      }
    }
    // A miss for the same logical identity may already be creating its
    // session. Do not start a second factory call; the caller can retry after
    // the in-flight turn publishes the binding.
    if (this.creatingSubagents.has(logical_subagent_id)) {
      throw new SidecarError(
        'logical subagent session creation is already in progress',
        -32002,
        { candidate: null, all_busy: true },
      );
    }
    // 2. Miss + full — throw HOT_SESSION_LIMIT_REACHED with `data.candidate`.
    //    Bug 1 fix: skip in-use sessions (currently running a turn) —
    //    evicting them would corrupt the in-flight task AND fail on the
    //    Rust side (no DB binding yet). If all sessions are in-use,
    //    `data.candidate = null` and `data.all_busy = true` so the executor
    //    knows NOT to attempt eviction.
    if (this.sessions.size + this.creatingSubagents.size >= this.maxHot) {
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
    this.creatingSubagents.set(logical_subagent_id, creationTaskId);
    try {
      const session = await factory(logical_subagent_id, opts);
      if (this.cancelledCreations.delete(logical_subagent_id)) {
        // Cancellation can arrive while the SDK session is still being
        // constructed, before subagentMap has an adapter id to look up.
        // Dispose the just-created session instead of publishing an
        // unbound pending ghost into the hot pool.
        void session.close();
        throw new SidecarError('turn cancelled before session creation completed', -32013);
      }
      const adapter_session_id = session.adapter_session_id;
      this.sessions.set(adapter_session_id, session);
      this.subagentMap.set(logical_subagent_id, adapter_session_id);
      this.pendingSessions.add(adapter_session_id);
      this.pendingSince.set(adapter_session_id, Date.now());
      if (acquireTurn) this.acquireTurn(adapter_session_id, creationTaskId);
      return { session, reused: false };
    } catch (error) {
      // Do not let a cancellation marker from a failed creation poison the
      // next turn for the same logical identity.
      this.cancelledCreations.delete(logical_subagent_id);
      throw error;
    } finally {
      this.creatingSubagents.delete(logical_subagent_id);
    }
  }

  /**
   * Acquire a session lease for a turn. The busy marker and task-aware lease
   * are installed inside `ensure()` before the result is returned to the RPC
   * handler, so eviction/cancel cannot race the hand-off.
   */
  ensureForTurn(
    logical_subagent_id: string,
    opts: CreateSessionOpts,
    createOverride?: SessionFactory,
    taskId?: string,
  ): Promise<{ session: PiSdkSession; reused: boolean }> {
    // Pass the lease request into ensure itself. For a hit, ensure executes
    // synchronously up to its resolved Promise and marks the session busy
    // before this function returns; this closes the event-loop gap where a
    // concurrent prepare_hibernate could reserve the session first.
    return this.ensure(logical_subagent_id, opts, createOverride, taskId, true);
  }

  /** Get a session by adapter_session_id. */
  get(adapter_session_id: string): PiSdkSession | undefined {
    return this.sessions.get(adapter_session_id);
  }

  /**
   * Abort an in-flight turn for `logical_subagent_id`. Looks up the hot
   * session by subagent id and calls `session.abort()`, which aborts the
   * underlying SDK HTTP request to the LLM provider — stopping token
   * generation. Activated sessions stay in the pool and can be reused for
   * subsequent turns. A newly-created pending session has no DB binding yet,
   * so a bounded fallback closes it if the SDK never settles the turn.
   *
   * Called by the `session.cancel` RPC handler. Returns `true` if a hot
   * session was found and abort was called, or if an in-flight session
   * creation was marked for cancellation. Returns `false` when no matching
   * turn exists for the subagent (the turn may have already completed or the
   * subagent was never seen by this sidecar).
   */
  async abortSession(logical_subagent_id: string, task_id?: string): Promise<boolean> {
    const adapter_session_id = this.subagentMap.get(logical_subagent_id);
    if (adapter_session_id === undefined) {
      const creatingTaskId = this.creatingSubagents.get(logical_subagent_id);
      if (task_id !== undefined && creatingTaskId !== task_id) {
        return false;
      }
      if (creatingTaskId !== undefined || this.creatingSubagents.has(logical_subagent_id)) {
        this.cancelledCreations.add(logical_subagent_id);
        return true;
      }
      return false;
    }
    const session = this.sessions.get(adapter_session_id);
    if (!session) return false;
    const lease = this.turnLeases.get(adapter_session_id);
    if (lease && !lease.started) {
      if (task_id !== undefined && lease.taskId !== task_id) {
        return false;
      }
      lease.cancelled = true;
      return true;
    }
    // `PiSdkSession.abort()` marks the turn as cancellation-pending before
    // its first await. Schedule the fallback immediately so a hung SDK
    // abort() cannot leave the session quarantined forever.
    const abortPromise = session.abort(task_id);
    if (!session.isCancellationPending()) {
      return false;
    }
    this.schedulePendingCancelCleanup(adapter_session_id, logical_subagent_id);
    const aborted = await abortPromise;
    if (!aborted) {
      this.clearPendingCancelCleanup(adapter_session_id);
      return false;
    }
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
    this.turnLeases.delete(adapter_session_id);
    this.evictingSessions.delete(adapter_session_id);
    this.clearPendingCancelCleanup(adapter_session_id);
    this.pendingSessions.delete(adapter_session_id);
    this.pendingSince.delete(adapter_session_id);
    // Dispose the underlying SDK session (fire-and-forget; dispose() is sync).
    void session.close();
  }

  /**
   * Close pending sessions that have exceeded the TTL. This is a safety
   * net for the case where Rust crashes between `turn_auto` success and
   * `session.activate` / `session.close` — without this, the orphaned
   * pending session would occupy a capacity slot indefinitely. In-flight
   * turns are never considered orphaned; their grace period starts when
   * `endTurn()` clears the busy marker. Called at the top of `ensure()`
   * before the capacity check.
   */
  evictExpiredPending(): void {
    if (this.pendingSessions.size === 0) return;
    const now = Date.now();
    for (const id of this.pendingSessions) {
      // A pending session can legitimately live longer than the TTL while
      // its turn is still executing. Only Rust's post-turn activate/close
      // handshake can make it an orphan, so never reclaim a busy session.
      if (this.busySessions.has(id)) continue;
      const since = this.pendingSince.get(id);
      if (since !== undefined && now - since > PENDING_TTL_MS) {
        logger.warn('session_pool.pending_expired', {
          adapter_session_id: id,
          age_ms: now - since,
          ttl_ms: PENDING_TTL_MS,
        });
        this.close(id);
      }
    }
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
   * Idempotent for already-active sessions: activating an active session
   * is a no-op. For unknown sessions (closed, never created, or lost to
   * sidecar restart), throws SESSION_NOT_FOUND so Rust can roll back the
   * DB binding — a successful activate on a missing session would create
   * a false `is_hot=1` binding with no backing sidecar session.
   */
  activate(adapter_session_id: string): void {
    if (this.lru.includes(adapter_session_id)) {
      // Already active — idempotent no-op.
      return;
    }
    if (!this.pendingSessions.has(adapter_session_id)) {
      // Session not in pool (closed, never created, or lost to restart).
      // Throw so Rust knows the binding must be rolled back — returning
      // success here would create a false `is_hot=1` ghost binding.
      throw new SidecarError(
        `session not found: ${adapter_session_id}`,
        -32001, // SESSION_NOT_FOUND
      );
    }
    this.pendingSessions.delete(adapter_session_id);
    this.clearPendingCancelCleanup(adapter_session_id);
    this.pendingSince.delete(adapter_session_id);
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
   * Reserve a turn before the handler's first await. This is distinct from
   * `beginTurn`: the pool busy marker protects eviction, while the lease lets
   * `session.cancel` cancel a request that has been admitted but has not yet
   * entered the SDK's `sendTurn()` state machine.
   */
  private acquireTurn(adapter_session_id: string, taskId?: string): void {
    this.beginTurn(adapter_session_id);
    this.turnLeases.set(adapter_session_id, {
      taskId,
      started: false,
      cancelled: false,
    });
  }

  /** Mark an admitted turn as entering the SDK call. */
  markTurnStarted(adapter_session_id: string, taskId?: string): boolean {
    const lease = this.turnLeases.get(adapter_session_id);
    if (!lease) return true;
    if (lease.taskId !== taskId) {
      return false;
    }
    if (lease.cancelled) return false;
    lease.started = true;
    return true;
  }

  /**
   * Mark a session as idle (turn completed). Paired with `beginTurn`.
   * Safe to call on a session that was already removed via `close()` —
   * the busy flag is cleaned up on close as well.
   */
  endTurn(adapter_session_id: string): void {
    this.busySessions.delete(adapter_session_id);
    this.turnLeases.delete(adapter_session_id);
    // Start the orphan grace period only after the in-flight turn finishes.
    // This gives Rust a full TTL window to persist the binding and call
    // `session.activate`, regardless of how long the model call took.
    if (this.pendingSessions.has(adapter_session_id)) {
      this.pendingSince.set(adapter_session_id, Date.now());
    }
  }

  /**
   * Reserve an idle session for the prepare_hibernate → close protocol.
   * `getLruCandidate()` calls this before returning a candidate from a
   * HOT_SESSION_LIMIT response; the prepare_hibernate RPC also calls it for
   * proactive pressure eviction, which selects candidates from the DB.
   *
   * Returns false when the session is missing or currently running a turn.
   * Re-claiming an existing reservation is idempotent so concurrent Rust
   * evictors can safely race and let the DB CAS distinguish the winner.
   */
  reserveForEviction(adapter_session_id: string): boolean {
    this.releaseExpiredEvictions();
    if (this.evictingSessions.has(adapter_session_id)) return true;
    if (!this.sessions.has(adapter_session_id)) return false;
    if (this.pendingSessions.has(adapter_session_id)) return false;
    if (this.busySessions.has(adapter_session_id)) return false;
    this.evictingSessions.set(adapter_session_id, Date.now());
    return true;
  }

  /** Release an eviction reservation when the RPC cannot proceed. */
  releaseEviction(adapter_session_id: string): void {
    this.evictingSessions.delete(adapter_session_id);
  }

  /**
   * Schedule a one-shot fallback for a cancelled session. The normal path is
   * still turn_auto's finally block; this only handles SDKs whose prompt
   * promise never settles after abort. A session remains quarantined until it
   * settles; if it does not, this closes it so a replacement turn can start.
   */
  private schedulePendingCancelCleanup(
    adapter_session_id: string,
    logical_subagent_id: string,
  ): void {
    if (this.pendingCancelCleanup.has(adapter_session_id)) return;
    const timer = setTimeout(() => {
      this.pendingCancelCleanup.delete(adapter_session_id);
      const session = this.sessions.get(adapter_session_id);
      if (!session || !session.isCancellationPending()) return;
      logger.warn('session_pool.pending_cancel_cleanup', {
        adapter_session_id,
        logical_subagent_id,
        grace_ms: PENDING_CANCEL_GRACE_MS,
      });
      this.close(adapter_session_id);
    }, PENDING_CANCEL_GRACE_MS);
    this.pendingCancelCleanup.set(adapter_session_id, timer);
  }

  /** Cancel a scheduled fallback when the session reaches any terminal state. */
  private clearPendingCancelCleanup(adapter_session_id: string): void {
    const timer = this.pendingCancelCleanup.get(adapter_session_id);
    if (timer !== undefined) {
      clearTimeout(timer);
      this.pendingCancelCleanup.delete(adapter_session_id);
    }
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
    this.releaseExpiredEvictions();
    for (let i = this.lru.length - 1; i >= 0; i--) {
      const id = this.lru[i]!;
      if (this.busySessions.has(id)) continue;
      if (this.evictingSessions.has(id)) continue;
      this.evictingSessions.set(id, Date.now());
      return id;
    }
    return undefined;
  }

  private releaseExpiredEvictions(): void {
    if (this.evictingSessions.size === 0) return;
    const cutoff = Date.now() - EVICTION_RESERVATION_TTL_MS;
    for (const [id, reservedAt] of this.evictingSessions) {
      if (reservedAt <= cutoff) {
        logger.warn('session_pool.eviction_reservation_expired', {
          adapter_session_id: id,
          age_ms: Date.now() - reservedAt,
          ttl_ms: EVICTION_RESERVATION_TTL_MS,
        });
        this.evictingSessions.delete(id);
      }
    }
  }
}
