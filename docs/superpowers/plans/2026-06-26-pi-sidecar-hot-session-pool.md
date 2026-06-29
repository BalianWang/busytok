# Pi Sidecar Hot Session Pool + LRU Eviction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the sidecar-side hot session pool with LRU tracking and the busytok-service-driven eviction flow so that hot sessions are reused when possible and the LRU session is evicted (prepare_hibernate → write memory → close) when the pool is full.

**Architecture:** The TS sidecar gains a `SessionPool` class (spec §5.2) that tracks `adapter_session_id` ↔ `logical_subagent_id` mappings with an LRU ordering. `session.turn_auto` is refactored to go through `pool.ensure()` first — hit returns `session_reused: true`, miss-with-capacity creates a new session, miss-full throws `HOT_SESSION_LIMIT_REACHED` (-32002) with `data.candidate` naming the LRU session. The Rust `SidecarTaskExecutor` catches this error, drives the eviction flow (prepare_hibernate → persist memory → close → retry turn_auto) end-to-end. The §3.3 invariant is preserved by reusing the existing atomic `commit_hibernate_binding_and_status` store function for the evicted binding.

**Tech Stack:** Rust (tokio, rusqlite, tracing, serde_json), TypeScript (Node.js, strict mode), JSON-RPC 2.0 over stdio, SQLite.

## Global Constraints

- **Protocol version:** `PROTOCOL_VERSION = 1` (spec §4.1) — unchanged.
- **Error code `HOT_SESSION_LIMIT_REACHED`:** `-32002` (spec §3.2), response `data.candidate` = LRU `adapter_session_id`.
- **§3.3 invariant:** `status='hot'` iff `is_hot=1, status='hot'` binding exists — eviction MUST use the atomic `commit_hibernate_binding_and_status` store function, never separate updates.
- **Sidecar never writes SQLite** (spec §4.4) — all DB mutations are in busytok-service.
- **No `std::sync::Mutex` held across `.await`** (project convention) — DB lock is acquired in scoped blocks, released before RPC calls.
- **`tracing` event codes** use the `subagent.*` namespace (project convention): `subagent.session.evicted`, `subagent.session.reused`, `subagent.session.eviction_failed`.
- **Resource event types** from spec §3.1: `session_hot` (new), `session_hibernate` (existing).
- **Coverage:** Per-crate `busytok-subagent` ≥ 90% (hard gate — Plan 3 adds comprehensive tests for session pool + eviction). Workspace ≥ 82% (hard gate — matches current reality; the gap to 85% is in out-of-scope crates, not `busytok-subagent`). 85% remains the workspace aspiration as other crates backfill coverage. TS sidecar: all new code must have unit tests.
- **Config:** `max_hot_sessions` default 3 (spec §8.2), passed to sidecar via `BUSYTOK_SIDECAR_MAX_HOT_SESSIONS` env var.
- **Mock sidecar fixture** (`mock-sidecar.sh`) must be extended, not replaced — existing tests depend on its current behavior.

---

## File Structure

### TS Sidecar (`apps/pi-sidecar/src/`)

| File | Responsibility | Action |
|------|---------------|--------|
| `session_pool.ts` | `SessionPool` class — tracks sessions, LRU order, ensure/create/close/evict | **Create** |
| `pi_session.ts` | `PiSession` interface — represents a single adapter session handle | **Create** |
| `handlers/turn_auto.ts` | Refactor to go through `pool.ensure()` | **Modify** |
| `handlers/prepare_hibernate.ts` | Real implementation — compact session, return memory delta | **Modify** |
| `handlers/close.ts` | Real implementation — remove session from pool | **Modify** |
| `handlers/health.ts` | Report real `sessions` count from pool | **Modify** |
| `types.ts` | Add `PrepareHibernateParams`, `PrepareHibernateResult`, `CloseParams`, `CloseResult`, `MemoryDelta` | **Modify** |
| `main.ts` | Wire new handlers, instantiate `SessionPool` | **Modify** |
| `tests/session_pool.test.ts` | `SessionPool` unit tests (vitest) | **Create** |

### Rust (`crates/busytok-subagent/src/`)

| File | Responsibility | Action |
|------|---------------|--------|
| `sidecar/supervisor.rs` | Add `SidecarHandle::prepare_hibernate()`, `SidecarHandle::close()` | **Modify** |
| `sidecar/executor.rs` | Catch `HOT_SESSION_LIMIT_REACHED`, drive eviction, retry | **Modify** |
| `sidecar/config.rs` | Add `max_hot_sessions: u32` field | **Modify** |
| `sidecar/mod.rs` | Extend `SidecarError::Application` to carry `Option<Value>` data field | **Modify** |
| `sidecar/client.rs` | Pass `SidecarRpcError.data` into `Application` variant | **Modify** |
| `error.rs` | Add `SubagentError::HotSessionLimit { candidate: String }` variant; update `From<SidecarError>` | **Modify** |

### Rust (`crates/busytok-store/src/`)

| File | Responsibility | Action |
|------|---------------|--------|
| `subagent_queries.rs` | Add `find_hot_binding_by_session(conn, adapter_session_id, harness)` and `write_hot_summary(conn, subagent_id, hot_summary)` | **Modify** |
| `db.rs` | Add `subagent_find_hot_binding_by_session` and `subagent_write_hot_summary` wrappers | **Modify** |

### Rust (`crates/busytok-config/src/`)

| File | Responsibility | Action |
|------|---------------|--------|
| `lib.rs` | `SubagentPiSidecarConfig.max_hot_sessions` already exists (default 3) — no change needed | — |

### Tests

| File | Responsibility | Action |
|------|---------------|--------|
| `crates/busytok-subagent/tests/fixtures/mock-sidecar.sh` | Add `BUSYTOK_MOCK_HOT_SESSION_LIMIT=N` and `BUSYTOK_MOCK_CLOSE_FAILS=1` env vars | **Modify** |
| `crates/busytok-subagent/tests/sidecar_supervisor.rs` | Add session reuse + eviction tests | **Modify** |
| `crates/busytok-subagent/tests/sidecar_executor.rs` | Add eviction-driven retry tests | **Modify** |
| `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs` | Add e2e eviction test | **Modify** |
| `crates/busytok-store/tests/subagent_queries.rs` | Add `find_hot_binding_by_session` unit tests | **Modify** |
| `apps/pi-sidecar/tests/session_pool.test.ts` | `SessionPool` unit tests (vitest) | **Create** |
| `apps/pi-sidecar/tests/turn_auto.test.ts` | `turn_auto` pool integration tests (vitest) | **Create** |

---

## Task 1: Store — LRU hot binding query

**Files:**
- Modify: `crates/busytok-store/src/subagent_queries.rs`
- Modify: `crates/busytok-store/src/db.rs`
- Test: `crates/busytok-store/tests/subagent_queries.rs`

**Interfaces:**
- Produces: `find_hot_binding_by_session(conn: &Connection, adapter_session_id: &str, harness: &str) -> Result<Option<SubagentHarnessBindingRow>>` — finds a hot binding by session ID. Used by Task 5 to locate the binding during eviction (the candidate comes from the RPC error's `data.candidate`, not a DB query).
- Produces: `write_hot_summary(conn: &Connection, subagent_id: &str, hot_summary: &str) -> Result<()>` — writes just the hot_summary field of a subagent's memory row. Used by Task 5 to persist the memory delta from prepare_hibernate.

- [ ] **Step 1: Write the failing tests for find_hot_binding_by_session**

Add to `crates/busytok-store/tests/subagent_queries.rs`:

```rust
#[test]
fn find_hot_binding_by_session_returns_binding_for_known_session() {
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow {
        id: "sub_a".into(), name: "a".into(), project_id: "p".into(),
        repo_path: "/r".into(), repo_hash: "h".into(), branch: None, intent: None,
        default_profile: "pi/search-cheap".into(), default_model: None,
        status: "hot".into(), created_at_ms: now, updated_at_ms: now, last_active_at_ms: Some(now),
    }).unwrap();
    db.subagent_commit_hot_binding_and_status(&SubagentHarnessBindingRow {
        id: "bind_a".into(), subagent_id: "sub_a".into(), harness: "pi".into(),
        adapter_session_id: Some("sess_a".into()), adapter_process_id: None,
        is_hot: 1, status: "hot".into(), created_at_ms: now,
        last_used_at_ms: Some(now), closed_at_ms: None, detail_json: None,
    }, "sub_a").unwrap();

    let binding = db.subagent_find_hot_binding_by_session("sess_a", "pi").unwrap();
    assert!(binding.is_some());
    let binding = binding.unwrap();
    assert_eq!(binding.subagent_id, "sub_a");
    assert_eq!(binding.adapter_session_id.as_deref(), Some("sess_a"));
}

#[test]
fn find_hot_binding_by_session_returns_none_for_unknown_session() {
    let db = Database::open_in_memory().unwrap();
    let result = db.subagent_find_hot_binding_by_session("nonexistent", "pi").unwrap();
    assert!(result.is_none());
}

#[test]
fn find_hot_binding_by_session_excludes_closed_bindings() {
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow {
        id: "sub_b".into(), name: "b".into(), project_id: "p".into(),
        repo_path: "/r".into(), repo_hash: "h".into(), branch: None, intent: None,
        default_profile: "pi/search-cheap".into(), default_model: None,
        status: "warm".into(), created_at_ms: now, updated_at_ms: now, last_active_at_ms: Some(now),
    }).unwrap();
    // Insert a closed (is_hot=0) binding — should NOT be found
    db.subagent_commit_hibernate_binding_and_status(&SubagentHarnessBindingRow {
        id: "bind_b".into(), subagent_id: "sub_b".into(), harness: "pi".into(),
        adapter_session_id: Some("sess_b".into()), adapter_process_id: None,
        is_hot: 0, status: "closed".into(), created_at_ms: now,
        last_used_at_ms: Some(now), closed_at_ms: Some(now), detail_json: None,
    }, "sub_b", "warm").unwrap();

    let result = db.subagent_find_hot_binding_by_session("sess_b", "pi").unwrap();
    assert!(result.is_none(), "closed bindings must not be returned");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p busytok-store --test subagent_queries find_hot_binding_by_session`
Expected: FAIL — `subagent_find_hot_binding_by_session` method not found on `Database`.

- [ ] **Step 3: Implement find_hot_binding_by_session + write_hot_summary**

Add to `crates/busytok-store/src/subagent_queries.rs`:

```rust
/// Find a hot binding by adapter_session_id and harness.
/// Used by the eviction flow to locate the binding for a specific session.
pub fn find_hot_binding_by_session(
    conn: &Connection,
    adapter_session_id: &str,
    harness: &str,
) -> Result<Option<SubagentHarnessBindingRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, harness, adapter_session_id, adapter_process_id, \
                is_hot, status, created_at_ms, last_used_at_ms, closed_at_ms, detail_json \
         FROM subagent_harness_bindings \
         WHERE adapter_session_id = ?1 AND harness = ?2 AND is_hot = 1",
    )?;
    let row_opt = stmt
        .query_row(params![adapter_session_id, harness], |row| {
            Ok(SubagentHarnessBindingRow {
                id: row.get(0)?,
                subagent_id: row.get(1)?,
                harness: row.get(2)?,
                adapter_session_id: row.get(3)?,
                adapter_process_id: row.get(4)?,
                is_hot: row.get(5)?,
                status: row.get(6)?,
                created_at_ms: row.get(7)?,
                last_used_at_ms: row.get(8)?,
                closed_at_ms: row.get(9)?,
                detail_json: row.get(10)?,
            })
        })
        .ok();
    Ok(row_opt)
}

/// Write just the `hot_summary` field of a subagent's memory row.
/// Used by the eviction flow to persist the memory delta returned by
/// `session.prepare_hibernate`. Mirrors `SubagentManager::write_hot_summary`
/// but lives in the store layer so the executor can call it directly.
pub fn write_hot_summary(
    conn: &Connection,
    subagent_id: &str,
    hot_summary: &str,
) -> Result<()> {
    // UPSERT memory row with just hot_summary (other fields unchanged).
    // Mirrors the manager's write_hot_summary pattern: get-or-create, update
    // hot_summary, upsert.
    let existing: Option<SubagentMemoryRow> = conn
        .query_row(
            "SELECT id, subagent_id, hot_summary, long_summary, key_files_json, \
                    decisions_json, attempts_json, open_questions_json, artifact_refs_json, \
                    last_compacted_at_ms, last_compacted_task_id, updated_at_ms \
             FROM subagent_memory WHERE subagent_id = ?1",
            params![subagent_id],
            |row| {
                Ok(SubagentMemoryRow {
                    id: row.get(0)?,
                    subagent_id: row.get(1)?,
                    hot_summary: row.get(2)?,
                    long_summary: row.get(3)?,
                    key_files_json: row.get(4)?,
                    decisions_json: row.get(5)?,
                    attempts_json: row.get(6)?,
                    open_questions_json: row.get(7)?,
                    artifact_refs_json: row.get(8)?,
                    last_compacted_at_ms: row.get(9)?,
                    last_compacted_task_id: row.get(10)?,
                    updated_at_ms: row.get(11)?,
                })
            },
        )
        .ok();
    let now = busytok_domain::now_ms();
    match existing {
        Some(mut mem) => {
            mem.hot_summary = Some(hot_summary.to_string());
            mem.updated_at_ms = now;
            conn.execute(
                "UPDATE subagent_memory SET hot_summary = ?1, updated_at_ms = ?2 WHERE subagent_id = ?3",
                params![mem.hot_summary, mem.updated_at_ms, subagent_id],
            )?;
        }
        None => {
            conn.execute(
                "INSERT INTO subagent_memory (id, subagent_id, hot_summary, long_summary, \
                 key_files_json, decisions_json, attempts_json, open_questions_json, \
                 artifact_refs_json, last_compacted_at_ms, last_compacted_task_id, updated_at_ms) \
                 VALUES (?1, ?2, ?3, NULL, '[]', '[]', '[]', '[]', '[]', NULL, NULL, ?4)",
                params![
                    format!("mem_{subagent_id}"),
                    subagent_id,
                    hot_summary,
                    now,
                ],
            )?;
        }
    }
    Ok(())
}
```

Add to `crates/busytok-store/src/db.rs`:

```rust
pub fn subagent_find_hot_binding_by_session(
    &self,
    adapter_session_id: &str,
    harness: &str,
) -> Result<Option<SubagentHarnessBindingRow>> {
    let conn = self.conn();
    subagent_queries::find_hot_binding_by_session(conn, adapter_session_id, harness)
}

pub fn subagent_write_hot_summary(
    &self,
    subagent_id: &str,
    hot_summary: &str,
) -> Result<()> {
    let conn = self.conn();
    subagent_queries::write_hot_summary(conn, subagent_id, hot_summary)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p busytok-store --test subagent_queries`
Expected: PASS — all 3 tests pass for find_hot_binding_by_session.

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-store/src/subagent_queries.rs crates/busytok-store/src/db.rs crates/busytok-store/tests/subagent_queries.rs
git commit -m "feat(store): add find_hot_binding_by_session and write_hot_summary for eviction flow"
```

---

## Task 2: TS Sidecar — SessionPool class

**Files:**
- Create: `apps/pi-sidecar/src/session_pool.ts`
- Create: `apps/pi-sidecar/tests/session_pool.test.ts`
- Create: `apps/pi-sidecar/src/pi_session.ts`
- Modify: `apps/pi-sidecar/src/types.ts`

**Interfaces:**
- Produces: `SessionPool` class with `ensure(logical_subagent_id) → { adapter_session_id, reused }`, `get(adapter_session_id) → PiSession | undefined`, `close(adapter_session_id) → void`, `size() → number`, `toArray() → PiSession[]`. Throws `SidecarError(-32002)` with `data.candidate` when full.

- [ ] **Step 1: Write the failing test**

Create `apps/pi-sidecar/tests/session_pool.test.ts`. Uses vitest (consistent with existing `handlers.test.ts` and `rpc.test.ts`):

```typescript
import { describe, it, expect } from 'vitest';
import { SessionPool } from '../src/session_pool.js';
import { SidecarError } from '../src/errors.js';

describe('SessionPool', () => {
  it('ensure creates new session when under limit', () => {
    const pool = new SessionPool(3);
    const result = pool.ensure('sub-a', () => 'sess-1');
    expect(result.adapter_session_id).toBe('sess-1');
    expect(result.reused).toBe(false);
    expect(pool.size()).toBe(1);
  });

  it('ensure reuses existing session for same subagent', () => {
    const pool = new SessionPool(3);
    pool.ensure('sub-a', () => 'sess-1');
    const result = pool.ensure('sub-a', () => 'sess-2');
    expect(result.adapter_session_id).toBe('sess-1');
    expect(result.reused).toBe(true);
    expect(pool.size()).toBe(1);
  });

  it('ensure throws HOT_SESSION_LIMIT_REACHED with candidate when full', () => {
    const pool = new SessionPool(2);
    pool.ensure('sub-a', () => 'sess-1'); // LRU after next
    pool.ensure('sub-b', () => 'sess-2'); // MRU
    expect(() => pool.ensure('sub-c', () => 'sess-3')).toThrow(SidecarError);
    try {
      pool.ensure('sub-c', () => 'sess-3');
    } catch (e) {
      expect((e as SidecarError).code).toBe(-32002);
      expect((e as SidecarError).data).toEqual({ candidate: 'sess-1' });
    }
  });

  it('close removes session from pool', () => {
    const pool = new SessionPool(3);
    pool.ensure('sub-a', () => 'sess-1');
    pool.close('sess-1');
    expect(pool.size()).toBe(0);
    // Re-ensure creates a new session
    const result = pool.ensure('sub-a', () => 'sess-2');
    expect(result.adapter_session_id).toBe('sess-2');
    expect(result.reused).toBe(false);
  });

  it('LRU order updates on reuse', () => {
    const pool = new SessionPool(2);
    pool.ensure('sub-a', () => 'sess-1'); // LRU
    pool.ensure('sub-b', () => 'sess-2'); // MRU
    // Reuse sess-1 → it becomes MRU, sess-2 becomes LRU
    pool.ensure('sub-a', () => 'sess-1');
    expect(() => pool.ensure('sub-c', () => 'sess-3')).toThrow(SidecarError);
    try {
      pool.ensure('sub-c', () => 'sess-3');
    } catch (e) {
      expect((e as SidecarError).data).toEqual({ candidate: 'sess-2' });
    }
  });

  it('get returns session by adapter_session_id', () => {
    const pool = new SessionPool(3);
    pool.ensure('sub-a', () => 'sess-1');
    const session = pool.get('sess-1');
    expect(session).toBeDefined();
    expect(session!.logical_subagent_id).toBe('sub-a');
  });

  it('get returns undefined for unknown session', () => {
    const pool = new SessionPool(3);
    expect(pool.get('unknown')).toBeUndefined();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/pi-sidecar && pnpm test`
Expected: FAIL — module `../src/session_pool.js` not found.

- [ ] **Step 3: Implement PiSession interface**

Create `apps/pi-sidecar/src/pi_session.ts`:

```typescript
/**
 * Represents a single adapter session in the hot session pool.
 * A session is "hot" while it occupies a slot in the pool; closing it
 * frees the slot for reuse.
 */
export interface PiSession {
  adapter_session_id: string;
  logical_subagent_id: string;
  created_at_ms: number;
  last_used_at_ms: number;
}
```

- [ ] **Step 4: Add types to types.ts**

Add to `apps/pi-sidecar/src/types.ts`:

```typescript
export interface PrepareHibernateParams {
  adapter_session_id?: string;
  all?: boolean;
}

export interface MemoryDelta {
  hot_summary?: string;
  key_files?: string[];
  decisions?: string[];
  open_questions?: string[];
}

export interface PrepareHibernateResult {
  // Single-session path (adapter_session_id provided)
  memory_delta?: MemoryDelta | null;
  stats: Record<string, unknown>;
  // All-sessions path (all:true) — per-session breakdown so the Rust
  // shutdown/idle-exit path can persist each session's memory delta
  // individually (spec §5.4). Plan 3 returns the shape; Plan 4 wires the
  // real ContextBuilder memory and the Rust-side consumer.
  sessions?: HibernateSessionEntry[];
}

export interface HibernateSessionEntry {
  adapter_session_id: string;
  logical_subagent_id: string;
  memory_delta: MemoryDelta | null;
  stats: Record<string, unknown>;
}

export interface CloseParams {
  adapter_session_id: string;
}

export interface CloseResult {
  ok: boolean;
}
```

- [ ] **Step 5: Implement SessionPool**

Create `apps/pi-sidecar/src/session_pool.ts`:

```typescript
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
    // 2. Miss + full — throw HOT_SESSION_LIMIT_REACHED.
    // Step 6 adds `data: { candidate }` after extending SidecarError.
    if (this.sessions.size >= this.maxHot) {
      throw new SidecarError(
        'hot session limit reached',
        -32002,
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
```

- [ ] **Step 6: Extend SidecarError to carry `data` and update rpc.ts**

Modify `apps/pi-sidecar/src/errors.ts` — add optional `data` field:

```typescript
export class SidecarError extends Error {
  readonly data?: unknown;

  constructor(message: string, readonly code: number, data?: unknown) {
    super(message);
    this.name = 'SidecarError';
    this.data = data;
  }
}
```

Update `SessionPool.ensure()` to pass candidate in `data` (replace the throw block from Step 5):

```typescript
    if (this.sessions.size >= this.maxHot) {
      const candidate = this.lru[this.lru.length - 1];
      throw new SidecarError(
        'hot session limit reached',
        -32002,
        { candidate },
      );
    }
```

Modify `apps/pi-sidecar/src/rpc.ts` — update `writeError` to accept optional `data` and pass it from `SidecarError` in the catch block:

```typescript
  // In handleLine's catch block — replace the existing error handling:
    } catch (err: unknown) {
      // SidecarError carries a specific JSON-RPC code (and optional data);
      // anything else is surfaced as the default -32603 (internal error).
      if (err instanceof SidecarError) {
        this.writeError(req.id, err.code, err.message, err.data);
      } else {
        const message = err instanceof Error ? err.message : String(err);
        this.writeError(req.id, -32603, message);
      }
    }
```

```typescript
  // Update writeError signature to accept optional data:
  private writeError(id: number, code: number, message: string, data?: unknown): void {
    const err: JsonRpcError = { code, message, ...(data !== undefined ? { data } : {}) };
    const resp: JsonRpcResponse = { jsonrpc: '2.0', error: err, id };
    this.output.write(JSON.stringify(resp) + '\n');
  }
```

- [ ] **Step 7: Run test to verify it passes**

Run: `cd apps/pi-sidecar && pnpm test`
Expected: PASS — all 7 tests pass (plus existing handler/rpc tests).

- [ ] **Step 8: Commit**

```bash
git add apps/pi-sidecar/src/session_pool.ts apps/pi-sidecar/tests/session_pool.test.ts apps/pi-sidecar/src/pi_session.ts apps/pi-sidecar/src/types.ts apps/pi-sidecar/src/errors.ts apps/pi-sidecar/src/rpc.ts
git commit -m "feat(sidecar): add SessionPool with LRU tracking and HOT_SESSION_LIMIT_REACHED error"
```

---

## Task 3: TS Sidecar — Wire SessionPool into handlers

**Files:**
- Modify: `apps/pi-sidecar/src/handlers/turn_auto.ts`
- Modify: `apps/pi-sidecar/src/handlers/prepare_hibernate.ts`
- Modify: `apps/pi-sidecar/src/handlers/close.ts`
- Modify: `apps/pi-sidecar/src/handlers/health.ts`
- Modify: `apps/pi-sidecar/src/main.ts`
- Create: `apps/pi-sidecar/tests/turn_auto.test.ts`

**Interfaces:**
- Consumes: `SessionPool` from Task 2.
- Produces: Refactored `turn_auto` handler that goes through `pool.ensure()`, real `prepare_hibernate`/`close` handlers, `health` handler reporting real session count.

- [ ] **Step 1: Write the failing test for turn_auto reuse**

Create `apps/pi-sidecar/tests/turn_auto.test.ts`. Uses vitest (consistent with existing tests):

```typescript
import { describe, it, expect } from 'vitest';
import { SessionPool } from '../src/session_pool.js';
import { SidecarError } from '../src/errors.js';
import { turnAutoHandlerWithPool } from '../src/handlers/turn_auto.js';

describe('turn_auto with pool', () => {
  it('reuses session for same subagent', async () => {
    const pool = new SessionPool(3);
    const handler = turnAutoHandlerWithPool(pool);
    const params1 = {
      logical_subagent_id: 'sub-a',
      logical_subagent_name: 'a',
      cwd: '/tmp',
      profile: 'pi/search-cheap',
      prompt: 'do 1',
    };
    const result1 = await handler(params1);
    expect(result1.session_reused).toBe(false);

    const params2 = { ...params1, prompt: 'do 2' };
    const result2 = await handler(params2);
    expect(result2.session_reused).toBe(true);
    expect(result2.adapter_session_id).toBe(result1.adapter_session_id);
  });

  it('throws HOT_SESSION_LIMIT_REACHED when full', async () => {
    const pool = new SessionPool(1);
    const handler = turnAutoHandlerWithPool(pool);
    await handler(
      { logical_subagent_id: 'sub-a', cwd: '/tmp', profile: 'p', prompt: 'x' },
    );
    await expect(
      handler(
        { logical_subagent_id: 'sub-b', cwd: '/tmp', profile: 'p', prompt: 'x' },
      ),
    ).rejects.toThrow(SidecarError);
    try {
      await handler(
        { logical_subagent_id: 'sub-b', cwd: '/tmp', profile: 'p', prompt: 'x' },
      );
    } catch (e) {
      expect((e as SidecarError).code).toBe(-32002);
      expect((e as SidecarError).data?.candidate).toBeTruthy();
    }
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/pi-sidecar && pnpm test`
Expected: FAIL — `turnAutoHandlerWithPool` not exported.

- [ ] **Step 3: Refactor turn_auto.ts to use SessionPool**

Modify `apps/pi-sidecar/src/handlers/turn_auto.ts`:

```typescript
import { type TurnAutoParams, type TurnAutoResult } from '../types.js';
import type { RequestHandler } from '../rpc.js';
import { SidecarError } from '../errors.js';
import type { SessionPool } from '../session_pool.js';

let sessionCounter = 0;
function nextSessionId(): string {
  sessionCounter++;
  return `pi_sess_mock_${sessionCounter}`;
}

/**
 * turn_auto handler factory — takes a SessionPool so the pool is shared
 * across requests. The pool.ensure() call either reuses an existing
 * session or creates a new one; if the pool is full, it throws
 * HOT_SESSION_LIMIT_REACHED (-32002) with data.candidate.
 */
export function turnAutoHandlerWithPool(pool: SessionPool): RequestHandler {
  return async (params) => {
    const p = params as TurnAutoParams;
    if (!p.logical_subagent_id || !p.prompt) {
      throw new SidecarError('missing required fields', -32602);
    }
    const { adapter_session_id, reused } = pool.ensure(p.logical_subagent_id, nextSessionId);
    const result: TurnAutoResult = {
      adapter_session_id,
      session_reused: reused,
      status: 'completed',
      result: {
        task_summary: `[mock] turn completed for: ${p.prompt.slice(0, 80)}`,
      },
      usage: {
        model: p.model ?? 'deepseek-chat',
        provider: 'deepseek',
        input_tokens: p.prompt.length,
        output_tokens: 50,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_usd: 0.001,
      },
    };
    return result;
  };
}
```

- [ ] **Step 4: Implement prepare_hibernate with real pool**

Modify `apps/pi-sidecar/src/handlers/prepare_hibernate.ts`:

```typescript
import type { RequestHandler } from '../rpc.js';
import type { SessionPool } from '../session_pool.js';
import type { PrepareHibernateParams, PrepareHibernateResult, HibernateSessionEntry } from '../types.js';
import { SidecarError } from '../errors.js';

export function prepareHibernateHandlerWithPool(pool: SessionPool): RequestHandler {
  return async (params) => {
    const p = (params ?? {}) as PrepareHibernateParams;
    // `all: true` — compact all sessions (used by graceful shutdown / idle
    // exit, spec §5.4). Returns a per-session breakdown so the Rust
    // shutdown path can persist each session's memory delta individually.
    if (p.all) {
      const sessions = pool.toArray();
      const entries: HibernateSessionEntry[] = sessions.map((s) => ({
        adapter_session_id: s.adapter_session_id,
        logical_subagent_id: s.logical_subagent_id,
        // Mock memory — Plan 4 (ContextBuilder) wires real memory.
        memory_delta: { hot_summary: `[hibernate-all] session ${s.adapter_session_id} compacted` },
        stats: {},
      }));
      const result: PrepareHibernateResult = {
        stats: { sessions_compacted: entries.length },
        sessions: entries,
      };
      return result;
    }
    // Single session — compact and return memory delta
    if (!p.adapter_session_id) {
      throw new SidecarError('adapter_session_id required (or all:true)', -32602);
    }
    const session = pool.get(p.adapter_session_id);
    if (!session) {
      throw new SidecarError(`session not found: ${p.adapter_session_id}`, -32001);
    }
    const result: PrepareHibernateResult = {
      memory_delta: {
        hot_summary: `[hibernate] session ${p.adapter_session_id} compacted`,
      },
      stats: { subagent_id: session.logical_subagent_id },
    };
    return result;
  };
}
```

- [ ] **Step 5: Implement close with real pool**

Modify `apps/pi-sidecar/src/handlers/close.ts`:

```typescript
import type { RequestHandler } from '../rpc.js';
import type { SessionPool } from '../session_pool.js';
import type { CloseParams, CloseResult } from '../types.js';
import { SidecarError } from '../errors.js';

export function closeHandlerWithPool(pool: SessionPool): RequestHandler {
  return async (params) => {
    const p = (params ?? {}) as CloseParams;
    if (!p.adapter_session_id) {
      throw new SidecarError('adapter_session_id required', -32602);
    }
    const session = pool.get(p.adapter_session_id);
    if (!session) {
      throw new SidecarError(`session not found: ${p.adapter_session_id}`, -32001);
    }
    pool.close(p.adapter_session_id);
    const result: CloseResult = { ok: true };
    return result;
  };
}
```

- [ ] **Step 6: Update health handler to report real session count**

Modify `apps/pi-sidecar/src/handlers/health.ts`:

```typescript
import { type HealthResult } from '../types.js';
import type { RequestHandler } from '../rpc.js';
import type { SessionPool } from '../session_pool.js';

export function healthHandlerWithPool(pool: SessionPool): RequestHandler {
  return async () => {
    const result: HealthResult = {
      status: 'healthy',
      sessions: pool.size(),
      rss_mb: Math.round(process.memoryUsage().rss / 1024 / 1024),
    };
    return result;
  };
}
```

- [ ] **Step 7: Wire everything in main.ts**

Modify `apps/pi-sidecar/src/main.ts`:

```typescript
import { JsonRpcServer } from './rpc.js';
import { SessionPool } from './session_pool.js';
import { initializeHandler } from './handlers/initialize.js';
import { healthHandlerWithPool } from './handlers/health.js';
import { shutdownHandler } from './handlers/shutdown.js';
import { turnAutoHandlerWithPool } from './handlers/turn_auto.js';
import { prepareHibernateHandlerWithPool } from './handlers/prepare_hibernate.js';
import { closeHandlerWithPool } from './handlers/close.js';

const maxHot = parseInt(process.env.BUSYTOK_SIDECAR_MAX_HOT_SESSIONS ?? '3', 10);
const pool = new SessionPool(maxHot);
const server = new JsonRpcServer();

server.registerHandler('adapter.initialize', initializeHandler);
server.registerHandler('adapter.health', healthHandlerWithPool(pool));
server.registerHandler('adapter.shutdown', shutdownHandler);
server.registerHandler('session.turn_auto', turnAutoHandlerWithPool(pool));
server.registerHandler('session.prepare_hibernate', prepareHibernateHandlerWithPool(pool));
server.registerHandler('session.close', closeHandlerWithPool(pool));

server.onStop(() => process.exit(0));
server.start();
```

> **Note:** `session.ensure` and `session.turn` are future methods (spec §4.2).
> They are NOT included in Plan 3 — `session.turn_auto` is the MVP primary path
> and already covers ensure+turn in one call. Adding unused handlers would
> require LRU-touch plumbing (`getAndTouch`) that has no consumer yet. These
> will be added in a later plan when busytok-service actually splits ensure/turn.

- [ ] **Step 8: Run all TS tests**

Run: `cd apps/pi-sidecar && pnpm test && pnpm typecheck`
Expected: PASS — all tests pass (vitest), typecheck clean.

- [ ] **Step 9: Commit**

```bash
git add apps/pi-sidecar/src/handlers/ apps/pi-sidecar/src/main.ts apps/pi-sidecar/tests/turn_auto.test.ts
git commit -m "feat(sidecar): wire SessionPool into turn_auto/prepare_hibernate/close/health handlers"
```

---

## Task 4: Rust — SidecarError data field + SidecarHandle methods + error variant

**Files:**
- Modify: `crates/busytok-subagent/src/sidecar/mod.rs`
- Modify: `crates/busytok-subagent/src/sidecar/client.rs`
- Modify: `crates/busytok-subagent/src/sidecar/supervisor.rs`
- Modify: `crates/busytok-subagent/src/error.rs`

**Interfaces:**
- Produces: `SidecarError::Application(i32, String, Option<serde_json::Value>)` — now carries the JSON-RPC `data` field so callers can read `data.candidate` directly instead of querying the DB.
- Produces: `SidecarHandle::prepare_hibernate(adapter_session_id) -> Result<serde_json::Value, SidecarError>`, `SidecarHandle::close(adapter_session_id) -> Result<serde_json::Value, SidecarError>`, `SubagentError::HotSessionLimit { candidate: String }` variant.

- [ ] **Step 1: Extend SidecarError::Application to carry data**

Modify `crates/busytok-subagent/src/sidecar/mod.rs` — add a third field to the `Application` variant:

```rust
    #[error("sidecar application error [{0}]: {1}")]
    Application(i32, String, Option<serde_json::Value>),
```

Modify `crates/busytok-subagent/src/sidecar/client.rs` — pass `err.data` into the variant (line ~116):

```rust
            if let Some(err) = resp.error {
                return Err(SidecarError::Application(err.code, err.message, err.data));
            }
```

**Update existing tests that construct/match `SidecarError::Application` with 2 args** — changing the variant to 3 args is a breaking change for pattern matching. The following existing test files must be updated in this step:

- `crates/busytok-subagent/tests/sidecar_protocol.rs`:
  - Line ~49: `SidecarError::Application(SESSION_NOT_FOUND, "no such session".into())` → add `, None` as third arg
  - Line ~54: `SidecarError::Application(PROFILE_NOT_FOUND, "bad profile".into())` → add `, None`
  - Line ~57: `SidecarError::Application(TASK_TIMEOUT, "slow".into())` → add `, None`
  - Line ~115: `SidecarError::Application(code, label.into())` → add `, None` (inside the loop over unmatched codes — none of these carry data)
- `crates/busytok-subagent/tests/sidecar_client.rs`:
  - Line ~201: `SidecarError::Application(code, msg) =>` → update pattern to `SidecarError::Application(code, msg, _) =>` (the `_` discards the data field; the existing assertion only checks `code` and `msg`)

These are mechanical updates — no new test logic, just satisfying the new 3-field shape. Run `cargo test -p busytok-subagent --test sidecar_protocol --test sidecar_client` after this step to confirm the existing tests still pass.

- [ ] **Step 2: Add HotSessionLimit error variant + update From conversion**

Modify `crates/busytok-subagent/src/error.rs`:

```rust
    #[error("sidecar crashed: {0}")]
    SidecarCrashed(String),

    #[error("hot session limit reached, candidate: {candidate}")]
    HotSessionLimit { candidate: String },
}
```

Add to `code()`:

```rust
            SubagentError::SidecarCrashed(_) => "subagent.sidecar_crashed",
            SubagentError::HotSessionLimit { .. } => "subagent.hot_session_limit",
        }
    }
}
```

Update `From<SidecarError>` — the `Application` variant now has 3 fields; extract `candidate` from `data.candidate` for `HOT_SESSION_LIMIT_REACHED`:

```rust
            SidecarError::Application(code, msg, data) => {
                use crate::sidecar::protocol::*;
                match code {
                    SESSION_NOT_FOUND => SubagentError::NotFound(msg),
                    PROFILE_NOT_FOUND => SubagentError::ProfileNotFound(msg),
                    TASK_TIMEOUT => SubagentError::TaskTimeout,
                    HOT_SESSION_LIMIT_REACHED => {
                        // Extract candidate from the RPC error's data field.
                        // The sidecar is the hot-pool authority (spec §4.4) —
                        // its data.candidate names the LRU session to evict.
                        let candidate = data
                            .as_ref()
                            .and_then(|d| d.get("candidate"))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        SubagentError::HotSessionLimit { candidate }
                    }
                    _ => SubagentError::SidecarRpc(format!("[{code}] {msg}")),
                }
            }
```

- [ ] **Step 3: Add SidecarHandle methods**

Modify `crates/busytok-subagent/src/sidecar/supervisor.rs`:

```rust
impl SidecarHandle {
    pub async fn health(&self) -> Result<serde_json::Value, SidecarError> {
        self.supervisor.call_rpc("adapter.health", serde_json::json!({})).await
    }

    pub async fn turn_auto(&self, params: serde_json::Value) -> Result<serde_json::Value, SidecarError> {
        self.supervisor.call_rpc("session.turn_auto", params).await
    }

    /// Prepare a specific session for hibernate (spec §4.4 eviction flow).
    /// Returns `{ memory_delta, stats }`.
    pub async fn prepare_hibernate(
        &self,
        adapter_session_id: &str,
    ) -> Result<serde_json::Value, SidecarError> {
        self.supervisor
            .call_rpc(
                "session.prepare_hibernate",
                serde_json::json!({ "adapter_session_id": adapter_session_id }),
            )
            .await
    }

    /// Close a session (spec §4.4 eviction flow, final step).
    pub async fn close(
        &self,
        adapter_session_id: &str,
    ) -> Result<serde_json::Value, SidecarError> {
        self.supervisor
            .call_rpc(
                "session.close",
                serde_json::json!({ "adapter_session_id": adapter_session_id }),
            )
            .await
    }
}
```

- [ ] **Step 4: Run clippy + fmt**

Run: `cargo fmt --all && cargo clippy -p busytok-subagent -- -D warnings`
Expected: PASS — no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-subagent/src/sidecar/mod.rs crates/busytok-subagent/src/sidecar/client.rs crates/busytok-subagent/src/error.rs crates/busytok-subagent/src/sidecar/supervisor.rs
git commit -m "feat(subagent): carry RPC error data in SidecarError, add HotSessionLimit variant + prepare_hibernate/close handles"
```

---

## Task 5: Rust — Eviction driver in SidecarTaskExecutor

**Files:**
- Modify: `crates/busytok-subagent/src/sidecar/executor.rs`
- Modify: `crates/busytok-subagent/src/sidecar/config.rs`
- Modify: `crates/busytok-runtime/src/supervisor.rs` (pass max_hot_sessions to config)

**Interfaces:**
- Consumes: `find_hot_binding_by_session` (Task 1), `write_hot_summary` (Task 1), `SidecarHandle::prepare_hibernate/close` (Task 4), `SidecarError::Application` data field (Task 4), `commit_hibernate_binding_and_status` (existing).
- Produces: `SidecarTaskExecutor::execute` that catches `HOT_SESSION_LIMIT_REACHED`, extracts `candidate` from the RPC error's `data.candidate`, drives eviction, and retries `turn_auto` once. Also produces `SidecarTaskExecutor::with_db()` constructor and `PiSidecarSupervisor::config()` accessor.

- [ ] **Step 1: Add max_hot_sessions to SidecarConfig**

Modify `crates/busytok-subagent/src/sidecar/config.rs` — add the field to the struct:

```rust
pub struct SidecarConfig {
    pub node_binary: PathBuf,
    pub bundle_path: PathBuf,
    pub env: HashMap<String, String>,
    pub idle_exit_seconds: u64,
    pub health_interval: Duration,
    pub task_timeout: Duration,
    pub max_restart_attempts: u32,
    pub restart_backoff_base: Duration,
    pub harness_name: String,
    pub max_hot_sessions: u32,
}
```

In `resolve_sidecar_config`, replace `env: HashMap::new()` with an env map that includes `BUSYTOK_SIDECAR_MAX_HOT_SESSIONS`, and add `max_hot_sessions` to the returned struct:

```rust
    let mut env = HashMap::new();
    env.insert(
        "BUSYTOK_SIDECAR_MAX_HOT_SESSIONS".to_string(),
        settings.max_hot_sessions.to_string(),
    );
    Ok(SidecarConfig {
        node_binary,
        bundle_path,
        env,
        idle_exit_seconds: settings.idle_exit_seconds,
        health_interval: Duration::from_secs(30),
        task_timeout: Duration::from_secs(settings.task_timeout_seconds),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: settings.max_hot_sessions,
    })
```

**Update the existing test in `crates/busytok-subagent/tests/sidecar_config.rs`:** the test `resolve_sidecar_config_carries_timeouts_and_limits` currently asserts `assert!(cfg.env.is_empty());` (line 194). Since `resolve_sidecar_config` now populates `env` with `BUSYTOK_SIDECAR_MAX_HOT_SESSIONS`, that assertion will fail. Replace it:

```rust
    // env now carries the hot session limit (spec §8.2); API keys are added
    // at spawn in Plan 4.
    assert_eq!(
        cfg.env.get("BUSYTOK_SIDECAR_MAX_HOT_SESSIONS"),
        Some(&settings.max_hot_sessions.to_string()),
        "max_hot_sessions must be passed to the sidecar via env var"
    );
```

Also add an assertion for the new field after the existing `assert_eq!` lines:

```rust
    assert_eq!(cfg.max_hot_sessions, settings.max_hot_sessions);
```

- [ ] **Step 2: Update test helpers to include the new field**

In `crates/busytok-subagent/tests/sidecar_supervisor.rs`, add `max_hot_sessions: 3` to `mock_config()`:

```rust
fn mock_config() -> SidecarConfig {
    SidecarConfig {
        node_binary: PathBuf::from("bash"),
        bundle_path: mock_sidecar_script(),
        env: HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(3600),
        task_timeout: Duration::from_secs(5),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_millis(10),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
    }
}
```

In `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`, add `max_hot_sessions: 3` to `make_sidecar_config()`:

```rust
fn make_sidecar_config() -> SidecarConfig {
    SidecarConfig {
        node_binary: PathBuf::from("bash"),
        bundle_path: mock_sidecar_path(),
        env: HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(30),
        task_timeout: Duration::from_secs(30),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
    }
}
```

In `crates/busytok-subagent/tests/sidecar_executor.rs`, the existing `mock_sidecar_config_with_env` helper constructs `SidecarConfig` without the new `max_hot_sessions` field — it will fail to compile after Step 1. Add the field:

```rust
fn mock_sidecar_config_with_env(env: HashMap<String, String>) -> SidecarConfig {
    SidecarConfig {
        node_binary: PathBuf::from("bash"),
        bundle_path: mock_sidecar_script(),
        env,
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(3600),
        task_timeout: Duration::from_secs(10),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_millis(10),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
    }
}
```

The eviction tests in Step 3 and Task 7 call `mock_config()` in this file, but `sidecar_executor.rs` currently has no such helper (only `mock_sidecar_config_with_env`). Add a `mock_config()` convenience wrapper:

```rust
fn mock_config() -> SidecarConfig {
    mock_sidecar_config_with_env(HashMap::new())
}
```

- [ ] **Step 3: Write the failing eviction test**

Add to `crates/busytok-subagent/tests/sidecar_executor.rs`:

```rust
#[tokio::test]
async fn executor_evicts_lru_session_on_hot_limit_and_retries() {
    // Set up a sidecar with max_hot_sessions=1. First delegate fills the
    // pool. Second delegate (different subagent) triggers eviction:
    // executor catches HOT_SESSION_LIMIT_REACHED, calls prepare_hibernate +
    // close on the LRU session, persists memory + binding flip, then retries
    // turn_auto — which must now succeed.
    //
    // NOTE: The executor does NOT create subagent rows or persist the initial
    // hot binding — that's SubagentManager::delegate()'s job. This test
    // manually persists the binding after the first execute() to simulate
    // the post-delegate DB state the eviction flow depends on.
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env.insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let supervisor = PiSidecarSupervisor::new(cfg, Some(db.clone()));
    let executor = SidecarTaskExecutor::with_db(supervisor.clone(), db.clone());

    // First delegate — fills the pool (max_hot=1)
    let input1 = ExecutorInput {
        subagent_id: "sub-a".into(),
        subagent_name: "a".into(),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        model: None,
        prompt: "do 1".into(),
        timeout_seconds: None,
    };
    let out1 = executor.execute(&input1).await.expect("first delegate must succeed");
    assert_eq!(out1.status, TaskStatus::Completed);
    let sess_a = out1.adapter_session_id.expect("must have session id");

    // Manually persist the hot binding (simulating what SubagentManager::delegate()
    // does after a successful execute()). The eviction flow's find_hot_binding_by_session
    // query depends on this binding existing.
    {
        let db_guard = db.lock().unwrap();
        db_guard.subagent_upsert_logical(&SubagentLogicalSubagentRow {
            id: "sub-a".into(), name: "a".into(), project_id: "p".into(),
            repo_path: "/r".into(), repo_hash: "h".into(), branch: None, intent: None,
            default_profile: "pi/search-cheap".into(), default_model: None,
            status: "hot".into(), created_at_ms: 0, updated_at_ms: 0, last_active_at_ms: Some(0),
        }).unwrap();
        let now = busytok_domain::now_ms();
        db_guard.subagent_commit_hot_binding_and_status(&SubagentHarnessBindingRow {
            id: "bind_a".into(), subagent_id: "sub-a".into(), harness: "pi".into(),
            adapter_session_id: Some(sess_a), adapter_process_id: None,
            is_hot: 1, status: "hot".into(), created_at_ms: now,
            last_used_at_ms: Some(now), closed_at_ms: None, detail_json: None,
        }, "sub-a").unwrap();
    }

    // Second delegate — different subagent, triggers eviction
    let input2 = ExecutorInput {
        subagent_id: "sub-b".into(),
        subagent_name: "b".into(),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        model: None,
        prompt: "do 2".into(),
        timeout_seconds: None,
    };
    let out2 = executor.execute(&input2).await.expect("eviction + retry must succeed");
    assert_eq!(out2.status, TaskStatus::Completed);
    assert!(out2.adapter_session_id.is_some());

    // Verify: sub-a is now warm (evicted), with memory written by the
    // eviction flow's prepare_hibernate → write_hot_summary path.
    {
        let db_guard = db.lock().unwrap();
        let sub_a = db_guard.subagent_get_logical("sub-a").unwrap().unwrap();
        assert_eq!(sub_a.status, "warm", "evicted subagent must be warm");
        let mem = db_guard.subagent_get_memory("sub-a").unwrap();
        assert!(mem.is_some(), "evicted subagent must have memory row");
        assert!(mem.unwrap().hot_summary.is_some(), "hot_summary must be written");
    }

    supervisor.shutdown().await.unwrap();
}
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo test -p busytok-subagent --test sidecar_executor executor_evicts_lru_session_on_hot_limit_and_retries`
Expected: FAIL — eviction logic not implemented.

- [ ] **Step 5: Implement eviction driver in executor**

Modify `crates/busytok-subagent/src/sidecar/executor.rs`:

```rust
use std::sync::{Arc, Mutex};
use tracing::{error, info, warn};

use busytok_store::SubagentResourceEventRow;

use crate::error::SubagentError;
use crate::mock_executor::{ExecutorInput, ExecutorOutput, TaskExecutor};
use crate::models::{TaskStatus, TaskUsage};
use crate::sidecar::supervisor::PiSidecarSupervisor;
use crate::sidecar::{protocol::HOT_SESSION_LIMIT_REACHED, SidecarError};

pub struct SidecarTaskExecutor {
    supervisor: Arc<PiSidecarSupervisor>,
    db: Option<Arc<Mutex<busytok_store::Database>>>,
}

impl SidecarTaskExecutor {
    pub fn new(supervisor: Arc<PiSidecarSupervisor>) -> Self {
        Self { supervisor, db: None }
    }

    /// Construct with a DB handle for eviction flow (production path).
    pub fn with_db(supervisor: Arc<PiSidecarSupervisor>, db: Arc<Mutex<busytok_store::Database>>) -> Self {
        Self { supervisor, db: Some(db) }
    }
}

#[async_trait]
impl TaskExecutor for SidecarTaskExecutor {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        let handle = self.supervisor.ensure_started().await.map_err(sidecar_to_anyhow)?;
        let params = serde_json::json!({
            "logical_subagent_id": input.subagent_id,
            "logical_subagent_name": input.subagent_name,
            "cwd": input.cwd,
            "profile": input.profile,
            "model": input.model,
            "prompt": input.prompt,
            "timeout_ms": input.timeout_seconds.map(|s| s * 1000),
        });
        info!(
            event_code = "subagent.sidecar.turn_auto.start",
            subagent_id = %input.subagent_id,
            profile = %input.profile,
            "sending turn_auto to sidecar"
        );
        match handle.turn_auto(params.clone()).await {
            Ok(result) => Ok(parse_turn_auto_result(&result)),
            Err(SidecarError::Application(code, _msg, data)) if code == HOT_SESSION_LIMIT_REACHED => {
                info!(
                    event_code = "subagent.session.hot_limit_reached",
                    subagent_id = %input.subagent_id,
                    "hot session limit reached, driving eviction"
                );
                let candidate = extract_candidate_from_data(data.as_ref())?;
                self.evict_session(&candidate).await?;
                // Retry turn_auto after eviction
                let result = handle.turn_auto(params).await.map_err(|e| {
                    warn!(event_code = "subagent.sidecar.turn_auto.failed_after_eviction", error = %e);
                    sidecar_to_anyhow(e)
                })?;
                Ok(parse_turn_auto_result(&result))
            }
            Err(e) => {
                warn!(event_code = "subagent.sidecar.turn_auto.failed", error = %e);
                Err(sidecar_to_anyhow(e))
            }
        }
    }
}

/// Extract the LRU candidate `adapter_session_id` from the JSON-RPC error's
/// `data.candidate` field. The sidecar is the hot-pool authority (spec §4.4) —
/// it names the LRU session in the error response, so we read it directly
/// rather than querying the local DB.
fn extract_candidate_from_data(data: Option<&serde_json::Value>) -> anyhow::Result<String> {
    let candidate = data
        .and_then(|d| d.get("candidate"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!(
            "HOT_SESSION_LIMIT_REACHED error missing data.candidate — \
             sidecar protocol violation"
        ))?;
    Ok(candidate.to_string())
}

impl SidecarTaskExecutor {
    /// Drive the eviction flow for a single session (spec §4.4):
    /// 1. RPC: session.prepare_hibernate(adapter_session_id) → memory_delta
    /// 2. Persist: write memory delta + flip binding (atomic) + write session_hibernate event
    /// 3. RPC: session.close(adapter_session_id) — failure is fatal (see below)
    async fn evict_session(&self, adapter_session_id: &str) -> anyhow::Result<()> {
        let handle = self.supervisor.ensure_started().await.map_err(sidecar_to_anyhow)?;
        // 1. prepare_hibernate → get memory delta
        let hibernate_result = handle
            .prepare_hibernate(adapter_session_id)
            .await
            .map_err(|e| {
                warn!(event_code = "subagent.session.eviction_prepare_failed", error = %e);
                sidecar_to_anyhow(e)
            })?;
        let memory_delta = hibernate_result.get("memory_delta").cloned();
        let stats = hibernate_result.get("stats").cloned();

        // 2. Persist: write memory + flip binding (atomic) + event
        if let Some(db) = &self.db {
            let db_guard = db.lock().expect("db lock poisoned");
            let harness = self.supervisor.config().harness_name.clone();
            // Find the binding for this adapter_session_id
            let binding = db_guard
                .subagent_find_hot_binding_by_session(adapter_session_id, &harness)
                .map_err(|e| anyhow::anyhow!("find binding failed: {e}"))?
                .ok_or_else(|| anyhow::anyhow!(
                    "no hot binding found for adapter_session_id {adapter_session_id}"
                ))?;
            // Write memory delta (hot_summary) if present
            if let Some(delta) = &memory_delta {
                if !delta.is_null() {
                    let hot_summary = delta.get("hot_summary")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    db_guard
                        .subagent_write_hot_summary(&binding.subagent_id, hot_summary)
                        .map_err(|e| anyhow::anyhow!("write hot_summary failed: {e}"))?;
                }
            }
            // Atomic: flip binding (is_hot=0, status='closed') + logical status='warm'
            let now = busytok_domain::now_ms();
            let mut flipped = binding.clone();
            flipped.is_hot = 0;
            flipped.status = "closed".into();
            flipped.closed_at_ms = Some(now);
            db_guard
                .subagent_commit_hibernate_binding_and_status(&flipped, &binding.subagent_id, "warm")
                .map_err(|e| anyhow::anyhow!("commit hibernate binding failed: {e}"))?;
            // Write session_hibernate resource event
            db_guard
                .subagent_insert_resource_event(&SubagentResourceEventRow {
                    id: format!("re_{}", uuid::Uuid::new_v4()),
                    event_type: "session_hibernate".into(),
                    target_id: Some(binding.subagent_id.clone()),
                    rss_mb: None,
                    cpu_percent: None,
                    detail_json: Some(serde_json::to_string(&serde_json::json!({
                        "adapter_session_id": adapter_session_id,
                        "reason": "evicted",
                        "stats": stats,
                    })).unwrap_or_default()),
                    created_at_ms: now,
                })
                .map_err(|e| anyhow::anyhow!("insert resource event failed: {e}"))?;
            info!(
                event_code = "subagent.session.evicted",
                subagent_id = %binding.subagent_id,
                adapter_session_id = %adapter_session_id,
                "evicted LRU session"
            );
        }

        // 3. close — failure is FATAL. If the sidecar didn't release the
        //    slot, retrying turn_auto would hit HOT_SESSION_LIMIT_REACHED
        //    again and the sidecar/DB would diverge (DB says closed/warm,
        //    sidecar still holds the session hot). Propagate the error so
        //    the caller knows the pool is in an inconsistent state; a
        //    sidecar restart is the recovery path.
        if let Err(e) = handle.close(adapter_session_id).await {
            error!(
                event_code = "subagent.session.eviction_close_failed",
                adapter_session_id = %adapter_session_id,
                error = %e,
                "session.close failed during eviction — DB flipped but sidecar slot not released; \
                 aborting retry to avoid state divergence (sidecar restart may be needed)"
            );
            return Err(anyhow::anyhow!(
                "session.close failed during eviction for {adapter_session_id}: {e} \
                 — sidecar pool may be inconsistent, restart recommended"
            ));
        }
        Ok(())
    }
}

fn parse_turn_auto_result(result: &serde_json::Value) -> ExecutorOutput {
    let adapter_session_id = result
        .get("adapter_session_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let session_reused = result
        .get("session_reused")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let status_str = result
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("completed");
    let status = match status_str {
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed,
        "timeout" => TaskStatus::Failed,
        _ => TaskStatus::Completed,
    };
    let summary = result
        .pointer("/result/task_summary")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let usage = result
        .get("usage")
        .map(|u| TaskUsage {
            model: u.get("model").and_then(|v| v.as_str()).map(String::from),
            provider: u.get("provider").and_then(|v| v.as_str()).map(String::from),
            input_tokens: u.get("input_tokens").and_then(|v| v.as_i64()),
            output_tokens: u.get("output_tokens").and_then(|v| v.as_i64()),
            cache_read_tokens: u.get("cache_read_tokens").and_then(|v| v.as_i64()),
            cache_write_tokens: u.get("cache_write_tokens").and_then(|v| v.as_i64()),
            cost_usd: u.get("cost_usd").and_then(|v| v.as_f64()),
        })
        .unwrap_or_default();
    ExecutorOutput {
        adapter_session_id,
        session_reused,
        status,
        summary,
        usage,
    }
}

fn sidecar_to_anyhow(e: SidecarError) -> anyhow::Error {
    anyhow::Error::from(SubagentError::from(e))
}
```

- [ ] **Step 6: Add config accessor on supervisor**

Modify `crates/busytok-subagent/src/sidecar/supervisor.rs`:

```rust
impl PiSidecarSupervisor {
    /// Access the config (for executor to read harness_name, etc.)
    pub fn config(&self) -> &SidecarConfig {
        &self.config
    }
}
```

- [ ] **Step 7: Wire SidecarTaskExecutor::with_db in runtime**

Modify `crates/busytok-runtime/src/supervisor.rs` `construct_sidecar`. In the `Ok(sidecar_config)` branch, replace `SidecarTaskExecutor::new(Arc::clone(&sup))` with `SidecarTaskExecutor::with_db(Arc::clone(&sup), Arc::clone(db))`:

```rust
            Ok(sidecar_config) => {
                let sup = busytok_subagent::sidecar::PiSidecarSupervisor::new(
                    sidecar_config,
                    Some(Arc::clone(db)),
                );
                let exec: Arc<dyn busytok_subagent::mock_executor::TaskExecutor> = Arc::new(
                    busytok_subagent::sidecar::SidecarTaskExecutor::with_db(
                        Arc::clone(&sup),
                        Arc::clone(db),
                    ),
                );
                (exec, Some(sup), None)
            }
```

- [ ] **Step 8: Run test to verify it passes**

Run: `cargo test -p busytok-subagent --test sidecar_executor executor_evicts_lru_session_on_hot_limit_and_retries`
Expected: PASS.

- [ ] **Step 9: Run all tests + clippy + fmt**

Run: `cargo fmt --all && cargo clippy -p busytok-subagent -p busytok-runtime --tests -- -D warnings && cargo test -p busytok-subagent && cargo test -p busytok-runtime --test subagent_e2e_sidecar`
Expected: PASS — all tests pass, no warnings.

- [ ] **Step 10: Commit**

```bash
git add crates/busytok-subagent/src/sidecar/executor.rs crates/busytok-subagent/src/sidecar/config.rs crates/busytok-subagent/src/sidecar/supervisor.rs crates/busytok-runtime/src/supervisor.rs crates/busytok-subagent/tests/sidecar_executor.rs
git commit -m "feat(subagent): implement LRU eviction driver in SidecarTaskExecutor"
```

---

## Task 6: Mock sidecar — hot session limit fixture

**Files:**
- Modify: `crates/busytok-subagent/tests/fixtures/mock-sidecar.sh`

**Interfaces:**
- Produces: `BUSYTOK_MOCK_HOT_SESSION_LIMIT=N` env var — when the mock sidecar has N active sessions, the next `session.turn_auto` for a new subagent returns `HOT_SESSION_LIMIT_REACHED` (-32002) with `data.candidate`.
- Produces: `BUSYTOK_MOCK_CLOSE_FAILS=1` env var — `session.close` returns a JSON-RPC error (-32001) instead of ok. Used by Task 7 to test the fatal-close-failure eviction path.

- [ ] **Step 1: Extend mock-sidecar.sh**

Modify `crates/busytok-subagent/tests/fixtures/mock-sidecar.sh`:

```bash
#!/usr/bin/env bash
# ... existing header, add new env var doc:
#   BUSYTOK_MOCK_HOT_SESSION_LIMIT=N  When N sessions are active, the next
#                                     session.turn_auto for a NEW subagent
#                                     returns HOT_SESSION_LIMIT_REACHED
#                                     (-32002) with data.candidate.
#   BUSYTOK_MOCK_CLOSE_FAILS=1        session.close returns a JSON-RPC error
#                                     (-32001 SESSION_NOT_FOUND) instead of
#                                     ok. Used to test the fatal-close-failure
#                                     eviction path.
set -euo pipefail
CRASH_AFTER="${BUSYTOK_MOCK_CRASH_AFTER:--1}"
DELAY_MS="${BUSYTOK_MOCK_DELAY_MS:-0}"
EMPTY_SESSION="${BUSYTOK_MOCK_EMPTY_SESSION:-0}"
STDERR_LINES="${BUSYTOK_MOCK_STDERR_LINES:-0}"
HOT_LIMIT="${BUSYTOK_MOCK_HOT_SESSION_LIMIT:-0}"
CLOSE_FAILS="${BUSYTOK_MOCK_CLOSE_FAILS:-0}"
COUNT=0

# Track active sessions using parallel indexed arrays (portable to bash 3.2
# which does NOT support `declare -A`). SUB_IDS[i] maps to SESS_IDS[i].
# SESS_ORDER tracks LRU order: index 0 = oldest (LRU), last = newest (MRU).
SUB_IDS=()
SESS_IDS=()
SESS_ORDER=()
SESS_COUNTER=0

# Look up a subagent's session by iterating the parallel arrays.
# Echoes the session_id, or empty string if not found.
sub_to_sess_lookup() {
  local target="$1" i
  for i in "${!SUB_IDS[@]}"; do
    if [[ "${SUB_IDS[$i]}" == "$target" ]]; then
      printf '%s' "${SESS_IDS[$i]}"
      return 0
    fi
  done
  return 0  # not found, prints nothing
}

# Remove a subagent→session mapping by subagent_id.
sub_to_sess_remove_by_sub() {
  local target="$1" i
  for i in "${!SUB_IDS[@]}"; do
    if [[ "${SUB_IDS[$i]}" == "$target" ]]; then
      unset 'SUB_IDS[i]'
      unset 'SESS_IDS[i]'
      SUB_IDS=("${SUB_IDS[@]}")
      SESS_IDS=("${SESS_IDS[@]}")
      return 0
    fi
  done
}

# Remove a subagent→session mapping by session_id.
sub_to_sess_remove_by_sess() {
  local target="$1" i
  for i in "${!SESS_IDS[@]}"; do
    if [[ "${SESS_IDS[$i]}" == "$target" ]]; then
      unset 'SUB_IDS[i]'
      unset 'SESS_IDS[i]'
      SUB_IDS=("${SUB_IDS[@]}")
      SESS_IDS=("${SESS_IDS[@]}")
      return 0
    fi
  done
}

while IFS= read -r line; do
  COUNT=$((COUNT + 1))
  if [[ "$DELAY_MS" -gt 0 ]]; then
    awk -v ms="$DELAY_MS" 'BEGIN { system("sleep " ms/1000) }'
  fi
  if [[ "$STDERR_LINES" -gt 0 ]]; then
    for i in $(seq 1 "$STDERR_LINES"); do
      echo "[mock-sidecar stderr] line $i for msg $COUNT" >&2
    done
  fi
  METHOD=$(printf '%s' "$line" | sed -n 's/.*"method"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  ID=$(printf '%s' "$line" | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p')
  # Extract logical_subagent_id from params (for turn_auto)
  SUB_ID=$(printf '%s' "$line" | sed -n 's/.*"logical_subagent_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  # Extract adapter_session_id from params (for prepare_hibernate/close)
  SESS_ID_PARAM=$(printf '%s' "$line" | sed -n 's/.*"adapter_session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')

  case "$METHOD" in
    adapter.initialize)
      printf '{"jsonrpc":"2.0","result":{"protocol_version":1,"sidecar_version":"mock-1.0"},"id":%s}\n' "$ID"
      ;;
    adapter.health)
      printf '{"jsonrpc":"2.0","result":{"status":"healthy","sessions":%d,"rss_mb":42},"id":%s}\n' "${#SESS_ORDER[@]}" "$ID"
      ;;
    adapter.shutdown)
      printf '{"jsonrpc":"2.0","result":{"ok":true},"id":%s}\n' "$ID"
      exit 0
      ;;
    session.turn_auto)
      if [[ "$EMPTY_SESSION" == "1" ]]; then
        printf '{"jsonrpc":"2.0","result":{"adapter_session_id":"","session_reused":false,"status":"completed","result":{"task_summary":"mock turn completed"},"usage":{"model":"deepseek-chat","provider":"deepseek","input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.001}},"id":%s}\n' "$ID"
      else
        EXISTING_SESS=$(sub_to_sess_lookup "$SUB_ID")
        if [[ "$HOT_LIMIT" -gt 0 && "${#SESS_ORDER[@]}" -ge "$HOT_LIMIT" && -z "$EXISTING_SESS" ]]; then
          # Pool is full and this is a NEW subagent — return HOT_SESSION_LIMIT_REACHED
          CANDIDATE="${SESS_ORDER[0]}"  # LRU = oldest = index 0
          printf '{"jsonrpc":"2.0","error":{"code":-32002,"message":"hot session limit reached","data":{"candidate":"%s"}},"id":%s}\n' "$CANDIDATE" "$ID"
        elif [[ -n "$EXISTING_SESS" ]]; then
          # Reuse existing session for this subagent
          SESS="$EXISTING_SESS"
          REUSED="true"
          # Move to MRU (end of array): remove and re-append
          for i in "${!SESS_ORDER[@]}"; do
            if [[ "${SESS_ORDER[$i]}" == "$SESS" ]]; then
              unset 'SESS_ORDER[i]'
              SESS_ORDER=("${SESS_ORDER[@]}")
              break
            fi
          done
          SESS_ORDER+=("$SESS")
          printf '{"jsonrpc":"2.0","result":{"adapter_session_id":"%s","session_reused":%s,"status":"completed","result":{"task_summary":"mock turn completed"},"usage":{"model":"deepseek-chat","provider":"deepseek","input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.001}},"id":%s}\n' "$SESS" "$REUSED" "$ID"
        else
          # Create new session
          SESS_COUNTER=$((SESS_COUNTER + 1))
          SESS="pi_sess_mock_${SESS_COUNTER}"
          SUB_IDS+=("$SUB_ID")
          SESS_IDS+=("$SESS")
          SESS_ORDER+=("$SESS")
          REUSED="false"
          printf '{"jsonrpc":"2.0","result":{"adapter_session_id":"%s","session_reused":%s,"status":"completed","result":{"task_summary":"mock turn completed"},"usage":{"model":"deepseek-chat","provider":"deepseek","input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.001}},"id":%s}\n' "$SESS" "$REUSED" "$ID"
        fi
      fi
      ;;
    session.prepare_hibernate)
      printf '{"jsonrpc":"2.0","result":{"memory_delta":{"hot_summary":"hibernated"},"stats":{"adapter_session_id":"%s"}},"id":%s}\n' "$SESS_ID_PARAM" "$ID"
      ;;
    session.close)
      if [[ "$CLOSE_FAILS" == "1" ]]; then
        # Simulate a sidecar that fails to close the session — used to test
        # the fatal-close-failure eviction path (P1-2 fix).
        printf '{"jsonrpc":"2.0","error":{"code":-32001,"message":"session.close failed: adapter error"},"id":%s}\n' "$ID"
      else
        # Remove from pool
        for i in "${!SESS_ORDER[@]}"; do
          if [[ "${SESS_ORDER[$i]}" == "$SESS_ID_PARAM" ]]; then
            unset 'SESS_ORDER[i]'
            SESS_ORDER=("${SESS_ORDER[@]}")
            break
          fi
        done
        # Remove from subagent map
        sub_to_sess_remove_by_sess "$SESS_ID_PARAM"
        printf '{"jsonrpc":"2.0","result":{"ok":true},"id":%s}\n' "$ID"
      fi
      ;;
    *)
      printf '{"jsonrpc":"2.0","error":{"code":-32601,"message":"method not found: %s"},"id":%s}\n' "$METHOD" "$ID"
      ;;
  esac
  if [[ "$CRASH_AFTER" -ge 0 && "$COUNT" -ge "$CRASH_AFTER" ]]; then
    echo "mock-sidecar crashing after $CRASH_AFTER messages" >&2
    exit 1
  fi
done
```

- [ ] **Step 2: Run existing tests to verify no regressions**

Run: `cargo test -p busytok-subagent --test sidecar_supervisor`
Expected: PASS — all existing tests still pass (HOT_LIMIT defaults to 0 = disabled).

- [ ] **Step 3: Commit**

```bash
git add crates/busytok-subagent/tests/fixtures/mock-sidecar.sh
git commit -m "test(sidecar): extend mock-sidecar.sh with HOT_SESSION_LIMIT, CLOSE_FAILS, and session reuse"
```

---

## Task 7: Rust — Session reuse test + e2e eviction test

**Files:**
- Modify: `crates/busytok-subagent/tests/sidecar_supervisor.rs`
- Modify: `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`

- [ ] **Step 1: Add session reuse test to sidecar_supervisor.rs**

```rust
#[tokio::test]
async fn supervisor_turn_auto_reuses_session_for_same_subagent() {
    let mut cfg = mock_config();
    cfg.health_interval = Duration::from_secs(3600);
    let sup = PiSidecarSupervisor::new(cfg, None);
    let handle = sup.ensure_started().await.unwrap();

    // First turn_auto — creates a new session
    let params1 = serde_json::json!({
        "logical_subagent_id": "sub-a",
        "logical_subagent_name": "a",
        "cwd": "/tmp",
        "profile": "pi/search-cheap",
        "prompt": "do 1",
    });
    let result1 = handle.turn_auto(params1).await.unwrap();
    let sess1 = result1["adapter_session_id"].as_str().unwrap().to_string();
    assert_eq!(result1["session_reused"], false);

    // Second turn_auto — same subagent, must reuse the session
    let params2 = serde_json::json!({
        "logical_subagent_id": "sub-a",
        "logical_subagent_name": "a",
        "cwd": "/tmp",
        "profile": "pi/search-cheap",
        "prompt": "do 2",
    });
    let result2 = handle.turn_auto(params2).await.unwrap();
    let sess2 = result2["adapter_session_id"].as_str().unwrap().to_string();
    assert_eq!(sess1, sess2, "same subagent must reuse the same session");
    assert_eq!(result2["session_reused"], true);

    sup.shutdown().await.unwrap();
}
```

- [ ] **Step 2: Add e2e eviction test**

Add to `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`:

```rust
#[tokio::test]
async fn sidecar_e2e_eviction_releases_lru_and_retries() {
    // max_hot_sessions=1: first delegate fills the pool. Second delegate
    // (different subagent) triggers eviction: executor catches
    // HOT_SESSION_LIMIT_REACHED, drives prepare_hibernate → persist → close,
    // then retries turn_auto. The evicted subagent must end up 'warm'
    // (memory written), the new subagent 'hot'.
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let mut settings = make_sidecar_settings();
    settings.subagent.pi_sidecar.max_hot_sessions = 1;
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    let mut cfg = make_sidecar_config();
    cfg.max_hot_sessions = 1;
    cfg.env.insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    let supervisor = BusytokSupervisor::new_with_sidecar_config(db, paths, cfg);

    // 1. First delegate — fills the pool
    let resp1 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "evicted".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "do 1".to_string(),
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .unwrap();
    assert_eq!(resp1.status, "completed");
    let sub1 = resp1.subagent_id;

    // 2. Second delegate — triggers eviction
    let resp2 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "winner".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "do 2".to_string(),
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .unwrap();
    assert_eq!(resp2.status, "completed");
    let sub2 = resp2.subagent_id;

    // 3. Verify: sub1 is warm (evicted with memory), sub2 is hot
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        let s1 = db_guard.subagent_get_logical(&sub1).unwrap().unwrap();
        assert_eq!(
            s1.status, "warm",
            "evicted subagent must be warm (memory written during eviction)"
        );
        let s2 = db_guard.subagent_get_logical(&sub2).unwrap().unwrap();
        assert_eq!(s2.status, "hot", "new subagent must be hot");
    }

    // 4. Verify session_hibernate resource event was written for the eviction
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        let events = db_guard.subagent_list_resource_events(None, 100).unwrap();
        assert!(
            events.iter().any(|e| e.event_type == "session_hibernate"),
            "session_hibernate event must be written during eviction"
        );
    }

    supervisor.shutdown_sidecar().await;
    supervisor.shutdown_writer().await.unwrap();
}
```

- [ ] **Step 3: Add eviction failure path test**

Add to `crates/busytok-subagent/tests/sidecar_executor.rs`:

```rust
#[tokio::test]
async fn executor_eviction_fails_when_db_has_no_binding_for_candidate() {
    // The sidecar returns HOT_SESSION_LIMIT_REACHED with data.candidate=X,
    // but the DB has no hot binding for X (sidecar and busytok-service are
    // out of sync). The executor must error out rather than silently
    // succeeding or retrying indefinitely.
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    // HOT_LIMIT=1 means the sidecar's pool is full after 1 session.
    cfg.env.insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let supervisor = PiSidecarSupervisor::new(cfg, Some(db.clone()));
    let executor = SidecarTaskExecutor::with_db(supervisor.clone(), db.clone());

    // First delegate — fills the sidecar's pool (max_hot=1)
    let input1 = ExecutorInput {
        subagent_id: "sub-a".into(),
        subagent_name: "a".into(),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        model: None,
        prompt: "do 1".into(),
        timeout_seconds: None,
    };
    let _out1 = executor.execute(&input1).await.expect("first delegate must succeed");

    // NOTE: We deliberately do NOT persist the hot binding to the DB.
    // This simulates the out-of-sync state: the sidecar has session X in
    // its pool, but the DB has no hot binding for X. When the sidecar
    // returns data.candidate=X, evict_session's find_hot_binding_by_session
    // will return None.

    // Second delegate — triggers HOT_SESSION_LIMIT_REACHED with data.candidate,
    // but eviction can't find the binding in the DB → must error out.
    let input2 = ExecutorInput {
        subagent_id: "sub-b".into(),
        subagent_name: "b".into(),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        model: None,
        prompt: "do 2".into(),
        timeout_seconds: None,
    };
    let result = executor.execute(&input2).await;
    assert!(
        result.is_err(),
        "eviction must fail when the DB has no hot binding for the candidate"
    );
    let err = result.unwrap_err();
    assert!(
        format!("{err}").contains("no hot binding found"),
        "error should explain the sync failure, got: {err}"
    );

    supervisor.shutdown().await.unwrap();
}
```

- [ ] **Step 4: Add close-failure eviction test**

Add to `crates/busytok-subagent/tests/sidecar_executor.rs`. This test exercises the P1-2 fix: when `session.close` fails during eviction, the executor must abort (return Err) rather than continuing — preventing state divergence where the DB says closed but the sidecar still holds the session.

This test pre-seeds the DB with a hot binding (unlike Step 3), so add these imports at the top of the file if not already present:
```rust
use busytok_domain::now_ms;
use busytok_store::{SubagentHarnessBindingRow, SubagentLogicalSubagentRow};
```

```rust
#[tokio::test]
async fn executor_eviction_aborts_when_session_close_fails() {
    // The sidecar returns HOT_SESSION_LIMIT_REACHED with data.candidate=X.
    // The DB has a hot binding for X (so find_hot_binding_by_session succeeds),
    // prepare_hibernate succeeds, the binding is flipped to closed in the DB,
    // BUT session.close(X) fails (BUSYTOK_MOCK_CLOSE_FAILS=1).
    // The executor must return an error — the DB has been flipped to closed
    // but the sidecar still holds the session. The caller must not retry.
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let now = busytok_domain::now_ms();
    // Pre-seed sub-a with a hot binding. The mock sidecar generates session
    // IDs as "pi_sess_mock_${counter}" (see mock-sidecar.sh), so the first
    // session it creates will be "pi_sess_mock_1". We pre-seed this binding
    // because the executor does NOT persist hot bindings itself (that is the
    // SubagentManager's job) — without this seed, find_hot_binding_by_session
    // would return None and eviction would fail before reaching close.
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow {
        id: "sub-a".into(), name: "a".into(), project_id: "p".into(),
        repo_path: "/r".into(), repo_hash: "h".into(), branch: None, intent: None,
        default_profile: "pi/search-cheap".into(), default_model: None,
        status: "hot".into(), created_at_ms: now, updated_at_ms: now, last_active_at_ms: Some(now),
    }).unwrap();
    db.subagent_commit_hot_binding_and_status(&SubagentHarnessBindingRow {
        id: "bind-a".into(), subagent_id: "sub-a".into(), harness: "pi".into(),
        adapter_session_id: Some("pi_sess_mock_1".into()), adapter_process_id: None,
        is_hot: 1, status: "hot".into(), created_at_ms: now,
        last_used_at_ms: Some(now), closed_at_ms: None, detail_json: None,
    }, "sub-a").unwrap();

    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env.insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.env.insert("BUSYTOK_MOCK_CLOSE_FAILS".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let supervisor = PiSidecarSupervisor::new(cfg, Some(db.clone()));
    let executor = SidecarTaskExecutor::with_db(supervisor.clone(), db.clone());

    // First delegate — fills the sidecar's pool (max_hot=1), session pi_sess_mock_1.
    let input1 = ExecutorInput {
        subagent_id: "sub-a".into(),
        subagent_name: "a".into(),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        model: None,
        prompt: "do 1".into(),
        timeout_seconds: None,
    };
    let _out1 = executor.execute(&input1).await.expect("first delegate must succeed");

    // Second delegate — triggers HOT_SESSION_LIMIT_REACHED with
    // data.candidate=pi_sess_mock_1 (the LRU session).
    // Eviction: find_hot_binding_by_session(pi_sess_mock_1) → OK (pre-seeded),
    // prepare_hibernate → OK, commit_hibernate_binding_and_status → flips DB,
    // session.close(pi_sess_mock_1) → FAILS (BUSYTOK_MOCK_CLOSE_FAILS=1).
    // Executor must return an error, NOT retry.
    let input2 = ExecutorInput {
        subagent_id: "sub-b".into(),
        subagent_name: "b".into(),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        model: None,
        prompt: "do 2".into(),
        timeout_seconds: None,
    };
    let result = executor.execute(&input2).await;
    assert!(
        result.is_err(),
        "eviction must abort when session.close fails — state divergence risk"
    );
    let err = result.unwrap_err();
    let err_msg = format!("{err}");
    assert!(
        err_msg.contains("session.close failed"),
        "error should mention the close failure, got: {err_msg}"
    );

    supervisor.shutdown().await.unwrap();
}
```

- [ ] **Step 5: Run all tests**

Run: `cargo test -p busytok-subagent --test sidecar_supervisor && cargo test -p busytok-subagent --test sidecar_executor && cargo test -p busytok-runtime --test subagent_e2e_sidecar`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/busytok-subagent/tests/sidecar_supervisor.rs crates/busytok-subagent/tests/sidecar_executor.rs crates/busytok-runtime/tests/subagent_e2e_sidecar.rs
git commit -m "test(subagent): add session reuse, e2e eviction, eviction failure, and close-failure tests"
```

---

## Task 8: Coverage gate + cleanup

**Files:**
- Modify: `scripts/coverage.sh`
- Modify: `crates/busytok-subagent/tests/sidecar_config.rs`

- [ ] **Step 1: Raise per-crate gate to 90; keep workspace at 82**

Modify `scripts/coverage.sh`. The per-crate `busytok-subagent` gate rises from 89 to 90 (Plan 3 adds comprehensive session pool + eviction tests). The workspace gate stays at 82 — the gap to 85% is in out-of-scope crates (same finding as Plan 2), not in `busytok-subagent`. Raising the workspace gate here would force unrelated test backfill or make the gate unenforceable.

```bash
# Workspace gate stays at 82 — the gap to 85% is in out-of-scope crates.
# 85% remains the aspiration as other crates backfill coverage.
GATE="${COVERAGE_GATE:-82}"
# Per-crate gate: 90 (Plan 3 target — session pool + eviction fully covered)
cargo llvm-cov -p busytok-subagent --fail-under-lines 90
```

- [ ] **Step 2: Add config test for max_hot_sessions**

Add to `crates/busytok-subagent/tests/sidecar_config.rs`:

```rust
#[test]
fn resolve_sidecar_config_passes_max_hot_sessions() {
    let tmp = TempDir::new().unwrap();
    let paths = paths_for(&tmp);
    let runtime_dir = tmp.path().to_string_lossy().to_string();
    write_bundle(tmp.path());

    let mut settings = SubagentPiSidecarConfig::default();
    settings.node_runtime = "system".to_string();
    settings.system_node_path = "bash".to_string();
    settings.runtime_dir = Some(runtime_dir);
    settings.max_hot_sessions = 5;

    let cfg = resolve_sidecar_config(&settings, &paths).unwrap();
    assert_eq!(cfg.max_hot_sessions, 5);
    assert_eq!(
        cfg.env.get("BUSYTOK_SIDECAR_MAX_HOT_SESSIONS"),
        Some(&"5".to_string()),
        "max_hot_sessions must be passed to sidecar via env var"
    );
}
```

- [ ] **Step 3: Run coverage gate**

Run: `bash scripts/coverage.sh`
Expected: PASS — workspace ≥ 82%, per-crate `busytok-subagent` ≥ 90%.

- [ ] **Step 4: Commit**

```bash
git add scripts/coverage.sh crates/busytok-subagent/tests/sidecar_config.rs
git commit -m "test(coverage): raise per-crate gate to 90, add max_hot_sessions config test"
```

---

## Self-Review

### 1. Spec coverage

- **§5.2 SessionPool** — Task 2 implements the full class with `ensure()`, LRU tracking, `HOT_SESSION_LIMIT_REACHED` with `data.candidate`. ✅
- **§4.4 Eviction flow** — Task 5 implements the complete flow: `prepare_hibernate` → persist memory + flip binding (atomic) → `close` (fatal on failure) → retry. ✅
- **§3.3 invariant** — Eviction uses existing `commit_hibernate_binding_and_status` (atomic). ✅
- **§3.2 HOT_SESSION_LIMIT_REACHED (-32002)** — Task 4 extends `SidecarError::Application` to carry `data`; Task 5 extracts `candidate` from `data.candidate` directly. ✅
- **§4.2 session.ensure / session.turn** — NOT included in Plan 3. These are future methods (spec §4.2); `session.turn_auto` is the MVP primary path and covers ensure+turn in one call. Adding unused handlers would require LRU-touch plumbing with no consumer. Deferred to a later plan. ⚠️ (deferred)
- **§5.4 idle exit + graceful shutdown** — The TS `prepare_hibernate(all:true)` returns a per-session breakdown (`sessions[]` with `adapter_session_id`, `logical_subagent_id`, `memory_delta`) so the shape is ready for the Rust shutdown path to persist each session's memory. However, the Rust-side consumer (writing per-session memory to SQLite during `shutdown_internal`/idle-exit) is deferred to Plan 4 — it requires real memory from ContextBuilder, and the mock sidecar only returns synthetic deltas. The existing `shutdown_internal()` already calls `prepare_hibernate(all:true)` best-effort. ⚠️ (protocol shape in Plan 3; Rust consumer in Plan 4)
- **§8.2 max_hot_sessions default 3** — Already in config; Task 5 wires it to sidecar via env var. ✅
- **session_hot resource event** — Not explicitly written (spec lists it but the eviction flow uses `session_hibernate`). This is acceptable — `session_hot` is a future enhancement for when a session transitions warm→hot. ⚠️ (deferred)

### 2. Placeholder scan

No TBD/TODO/"implement later" found. All steps have complete code.

### 3. Type consistency

- `SessionPool.ensure()` returns `{ adapter_session_id, reused }` — consistent across TS and Rust parsing.
- `SidecarError::Application(i32, String, Option<Value>)` — 3-tuple in mod.rs, matched in executor and From conversion.
- `find_hot_binding_by_session` — defined in Task 1, consumed in Task 5.
- `write_hot_summary` — defined in Task 1, consumed in Task 5.
- `SidecarHandle::prepare_hibernate/close` — defined in Task 4, consumed in Task 5.
- `SidecarConfig.max_hot_sessions` — added in Task 5 Step 1, used in test helpers.
- `SidecarTaskExecutor::with_db()` — defined in Task 5, used in Task 5 test and Task 7 e2e test via runtime wiring.
- `PrepareHibernateResult.sessions` — per-session list for `all:true`; `HibernateSessionEntry` type defined in types.ts.

### 4. Architecture review

- **Reuses existing infrastructure**: `commit_hibernate_binding_and_status`, `SubagentResourceEventRow`, `SidecarHandle`, `SidecarRpcClient`, `tracing` event codes, mock-sidecar.sh fixture pattern.
- **TS test consistency**: new tests use vitest (same as existing `handlers.test.ts`/`rpc.test.ts`), live in `apps/pi-sidecar/tests/`, and run via `pnpm test` — no second test runner or colocated test style introduced.
- **No new migrations needed**: `last_used_at_ms` already exists on `subagent_harness_bindings`.
- **DB lock discipline**: `evict_session` acquires the DB lock in a scoped block for persistence, releases it before the `close` RPC call.
- **Error semantics**: `SidecarError::Application` now carries `Option<Value>` data (Task 4); the executor reads `data.candidate` directly — the sidecar is the hot-pool authority (spec §4.4). No DB LRU query needed; `find_hot_binding_by_session` is only used to look up the binding row for persistence, not candidate selection.
- **Eviction close failure is fatal**: if `session.close` fails, the executor returns an error instead of retrying — this prevents state divergence (DB flipped to closed but sidecar still holds the session). The caller can trigger a sidecar restart to recover. Tested by `executor_eviction_aborts_when_session_close_fails` (Task 7 Step 4) using `BUSYTOK_MOCK_CLOSE_FAILS=1`.
- **Portable bash**: mock-sidecar.sh uses parallel indexed arrays (not `declare -A`) for macOS bash 3.2 compatibility.
- **Eviction failure path**: Task 7 includes a test verifying that the executor errors out when the DB has no hot binding for the candidate (sidecar/DB sync loss).
- **Observability**: `subagent.session.hot_limit_reached`, `subagent.session.evicted`, `subagent.session.eviction_prepare_failed`, `subagent.session.eviction_close_failed` event codes.
