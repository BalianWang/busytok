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
  private readonly lru: string[] = [];                               // adapter_session_ids, MRU first

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
    if (this.sessions.size >= this.maxHot) {
      const candidate = this.getLruCandidate();
      throw new SidecarError('hot session limit reached', -32002, { candidate });
    }
    // 3. Miss + capacity — create a new session via the factory.
    const factory = createOverride ?? this.factory;
    const session = await factory(logical_subagent_id, opts);
    const adapter_session_id = session.adapter_session_id;
    this.sessions.set(adapter_session_id, session);
    this.subagentMap.set(logical_subagent_id, adapter_session_id);
    this.lru.unshift(adapter_session_id); // MRU at front
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
    // Dispose the underlying SDK session (fire-and-forget; dispose() is sync).
    void session.close();
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

  /** Get the LRU candidate for eviction (used in error data). */
  getLruCandidate(): string | undefined {
    return this.lru[this.lru.length - 1];
  }
}
