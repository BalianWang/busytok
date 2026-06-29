import type { PiSession } from './pi_session.js';
import { SidecarError } from './errors.js';

/**
 * Sidecar-side hot session pool with LRU tracking (spec §5.2).
 *
 * Tracks adapter_session_id ↔ logical_subagent_id mappings. When the pool
 * is full and a new session is requested, throws HOT_SESSION_LIMIT_REACHED
 * (-32002) with `data.candidate` naming the LRU session — busytok-service
 * then drives eviction via prepare_hibernate + close.
 *
 * LRU is maintained as an ordered array: index 0 = MRU, last = LRU.
 * On reuse, the session moves to the front. On close, it is removed.
 */
export class SessionPool {
  private readonly maxHot: number;
  private readonly sessions = new Map<string, PiSession>();       // adapter_session_id → session
  private readonly subagentMap = new Map<string, string>();        // logical_subagent_id → adapter_session_id
  private readonly lru: string[] = [];                             // adapter_session_ids, MRU first

  constructor(maxHot: number) {
    if (maxHot < 1) throw new Error(`maxHot must be >= 1, got ${maxHot}`);
    this.maxHot = maxHot;
  }

  /**
   * Ensure a hot session exists for `logical_subagent_id`.
   * - Hit: bump LRU, return `{ reused: true }`.
   * - Miss + capacity: call `createSession()`, add to pool, return `{ reused: false }`.
   * - Miss + full: throw HOT_SESSION_LIMIT_REACHED with `data.candidate`.
   */
  ensure(logical_subagent_id: string, createSession: () => string): { adapter_session_id: string; reused: boolean } {
    // 1. Hit — subagent already has a hot session
    const existing = this.subagentMap.get(logical_subagent_id);
    if (existing !== undefined) {
      this.touch(existing);
      return { adapter_session_id: existing, reused: true };
    }
    // 2. Miss + full — throw HOT_SESSION_LIMIT_REACHED with `data.candidate`.
    if (this.sessions.size >= this.maxHot) {
      const candidate = this.lru[this.lru.length - 1];
      throw new SidecarError(
        'hot session limit reached',
        -32002,
        { candidate },
      );
    }
    // 3. Miss + capacity — create new session
    const adapter_session_id = createSession();
    const now = Date.now();
    const session: PiSession = {
      adapter_session_id,
      logical_subagent_id,
      created_at_ms: now,
      last_used_at_ms: now,
    };
    this.sessions.set(adapter_session_id, session);
    this.subagentMap.set(logical_subagent_id, adapter_session_id);
    this.lru.unshift(adapter_session_id); // MRU at front
    return { adapter_session_id, reused: false };
  }

  /** Get a session by adapter_session_id. */
  get(adapter_session_id: string): PiSession | undefined {
    return this.sessions.get(adapter_session_id);
  }

  /** Close (remove) a session from the pool. */
  close(adapter_session_id: string): void {
    const session = this.sessions.get(adapter_session_id);
    if (!session) return;
    this.sessions.delete(adapter_session_id);
    this.subagentMap.delete(session.logical_subagent_id);
    const idx = this.lru.indexOf(adapter_session_id);
    if (idx >= 0) this.lru.splice(idx, 1);
  }

  /** Current number of hot sessions. */
  size(): number {
    return this.sessions.size;
  }

  /** All sessions as an array (for prepare_hibernate all). */
  toArray(): PiSession[] {
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
