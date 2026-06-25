# Busytok Pi Sidecar + Logical Subagent Runtime — Design Spec

**Date**: 2026-06-25
**Status**: Approved
**Branch**: TBD (implementation branch to be created from `main`)

---

## 1. Overview

Busytok extends its product scope from a passive token-usage audit dashboard to an active **task-level model-routing and subagent dispatch layer** for AI coding agents (Claude Code, Codex, etc.). A primary agent delegates sub-tasks to Busytok, which routes them to the appropriate harness + model based on task type, cost budget, context state, and resource pressure.

**Phase 1** implements only the Pi sidecar backend, validating four core capabilities:

1. Busytok can maintain long-lived **logical subagents** (DB entities, not processes).
2. Multiple context-related sub-tasks can reuse the same logical subagent.
3. State externalization and compute-storage separation eliminate per-subagent process residency.
4. Low-value sub-tasks can be offloaded to Pi + cheap models; future harnesses (Claude Code, Codex, OpenCode, Aider, Goose) plug into the same adapter interface.

**Terminology note**: This document uses "busytok-service" to refer to the Rust daemon process (`apps/service`). There is no separate `busytokd` binary — subagent functionality extends the existing busytok-service.

Architecture follows FastClaw principles: **state externalized, compute separated from storage, main process lightweight, plugin/worker process isolation, context restored on demand**.

---

## 2. Architecture

### 2.1 Layered Architecture

```
Claude Code / Codex / other primary Agent
        │
        ▼
busytok delegate CLI
        │
        ▼
busytok-control IPC server
        │
        ▼
busytok-service (apps/service, the only daemon)
  ├─ busytok-runtime
  │    ├─ ServiceApp / Runtime Lifecycle — owns SubagentManager
  │    ├─ Supervisor / Writer / Scan / Tail / Aggregator
  │    └─ Control dispatcher
  ├─ busytok-store
  │    ├─ SQLite: logical state / memory / tasks / usage / bindings
  │    └─ Artifact Store: logs / traces / patches / large outputs
  └─ busytok-config

        │
        ▼
busytok-subagent (NEW crate, managed by busytok-runtime)
  ├─ Manager — public API (delegate, list, show, hibernate)
  ├─ Router — profile → model/tools/timeout resolution
  ├─ ContextBuilder — compact context assembly
  ├─ MemoryUpdater — post-task memory merge + compaction
  ├─ ConcurrencyController — per-subagent task serialization
  ├─ ResourceMonitor / ResourcePolicy
  └─ sidecar/
       ├─ PiSidecarSupervisor — process lifecycle (spawn, health, crash recovery)
       ├─ JSON-RPC client — Busytok Adapter RPC over stdio
       └─ protocol types

        │
        ▼
 JSON-RPC over stdio

Pi sidecar subprocess (Node.js, on-demand)
  ├─ RPC server — Busytok Adapter RPC
  ├─ Pi SDK adapter — wraps @earendil-works/pi-coding-agent
  ├─ Session pool — ≤ max_hot_sessions
  ├─ Tool whitelist enforcement
  ├─ LRU tracking
  └─ Health check
```

### 2.2 Key Constraints

- **busytok-service is the only persistent daemon**. No separate process. This document uses "busytok-service" consistently; historical shorthand "busytokd" in design discussions means busytok-service.
- **`busytok-subagent` does NOT depend on `busytok-runtime`**. Runtime holds SubagentManager; subagent crate has no reverse dependency.
- **Pi sidecar is a subprocess, not embedded**. Node/V8 never enters the Rust main process.
- **SQLite is the source of truth** for all logical subagent state, tasks, memory, bindings, usage, and resource events. Pi session is hot cache only.
- **Pi sidecar is on-demand**: not started until the first Pi task arrives; idle TTL expiry → hibernate all sessions → exit process.
- **busytok-control is the IPC entry point only**; it dispatches to busytok-runtime and never touches the sidecar directly.

### 2.3 Crate Dependency Graph

```
apps/service ──▶ busytok-runtime, busytok-control
apps/cli     ──▶ busytok-control, busytok-runtime

busytok-runtime
  ├─▶ busytok-subagent    (NEW)
  ├─▶ busytok-store
  ├─▶ busytok-config
  ├─▶ busytok-events
  └─▶ busytok-protocol

busytok-subagent           (NEW)
  ├─▶ busytok-store
  ├─▶ busytok-config
  └─▶ tokio, serde, tracing, sysinfo (NEW workspace dep)

busytok-store
  └─▶ (no dependency on subagent)

busytok-control
  ├─▶ busytok-config
  ├─▶ busytok-events
  └─▶ busytok-protocol
  (does NOT depend on busytok-runtime or busytok-subagent)
```

NOTE: `busytok-runtime` does NOT depend on `busytok-control`. The control server runs in `apps/service` and dispatches to runtime via direct function calls, not via the control crate.

---

## 3. Data Model

### 3.1 Migration

- Single `busytok.db`. The current codebase has `SCHEMA_VERSION = 1` with `0001_baseline.sql`.
- New migration: `crates/busytok-store/migrations/0002_subagent.sql`, `SCHEMA_VERSION` → 2.
- The existing `baseline_single_migration()` test asserts `migrations().len() == 1`; update it to `== 2`.
- Migration added in `schema::migrations()` via `(2, include_str!("../migrations/0002_subagent.sql"))`.
- All new tables use `subagent_` prefix to avoid namespace collisions.

**Timestamp convention**: All new timestamp columns MUST use the `_ms` suffix (e.g. `created_at_ms`, `updated_at_ms`) to match the existing schema convention (`usage_events.created_at_ms`, `sessions.first_seen_at_ms`, etc.). This is not cosmetic — it distinguishes millisecond-epoch integers from second-based durations.

### 3.2 Tables

#### `subagent_logical_subagents`

```sql
CREATE TABLE subagent_logical_subagents (
  id TEXT PRIMARY KEY,              -- uuid v4
  name TEXT NOT NULL,               -- human-readable, unique within (project_id, repo_hash) scope
  project_id TEXT NOT NULL,         -- MVP: = repo_hash (derived via derive_project_hash)
  repo_path TEXT NOT NULL,
  repo_hash TEXT NOT NULL,
  branch TEXT,
  intent TEXT,
  default_profile TEXT NOT NULL,
  default_model TEXT,
  status TEXT NOT NULL DEFAULT 'cold'
    CHECK(status IN ('hot', 'warm', 'cold', 'deleted')),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  last_active_at_ms INTEGER
);

CREATE INDEX idx_subagent_logical_project
  ON subagent_logical_subagents(project_id, repo_hash, status);
CREATE INDEX idx_subagent_logical_last_active
  ON subagent_logical_subagents(last_active_at_ms);

-- Prevent duplicate active subagent names within the same repo scope
CREATE UNIQUE INDEX idx_subagent_unique_active_name
  ON subagent_logical_subagents(project_id, repo_hash, name)
  WHERE status != 'deleted';
```

#### `subagent_memory`

1:1 with logical subagent. Stores the **current** recoverable memory state only.

```sql
CREATE TABLE subagent_memory (
  id TEXT PRIMARY KEY,
  subagent_id TEXT NOT NULL UNIQUE,
  hot_summary TEXT,                 -- current_state_summary from most recent task
  long_summary TEXT,                -- compacted long-term summary
  key_files_json TEXT,              -- [{"path":"...","reason":"...","last_seen_at_ms":...,"score":N}]
  decisions_json TEXT,
  attempts_json TEXT,
  open_questions_json TEXT,         -- [{"question":"...","status":"open|resolved","created_at_ms":...,"last_seen_at_ms":...}]
  artifact_refs_json TEXT,
  last_compacted_at_ms INTEGER,
  last_compacted_task_id TEXT,
  updated_at_ms INTEGER NOT NULL,
  FOREIGN KEY(subagent_id) REFERENCES subagent_logical_subagents(id)
);
```

MVP uses 1:1 only. Future multi-version snapshots go to a separate `subagent_memory_snapshots` table.

#### `subagent_tasks`

```sql
CREATE TABLE subagent_tasks (
  id TEXT PRIMARY KEY,
  subagent_id TEXT NOT NULL,
  source_harness TEXT,              -- claude-code | codex | cli
  source_session_id TEXT,
  intent TEXT,
  profile TEXT NOT NULL,
  prompt TEXT,                       -- inline prompt; NULL when stored as artifact (see prompt_artifact_ref)
  prompt_artifact_ref TEXT,         -- set when prompt exceeds 32KB; then prompt is NULL or truncated preview
  output_schema_name TEXT,
  output_schema_version INTEGER DEFAULT 1,
  status TEXT NOT NULL DEFAULT 'queued'
    CHECK(status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
  CHECK(prompt IS NOT NULL OR prompt_artifact_ref IS NOT NULL),
  result_summary TEXT,
  result_json TEXT,
  error TEXT,
  created_at_ms INTEGER NOT NULL,
  started_at_ms INTEGER,
  completed_at_ms INTEGER,
  FOREIGN KEY(subagent_id) REFERENCES subagent_logical_subagents(id)
);

CREATE INDEX idx_subagent_tasks_subagent ON subagent_tasks(subagent_id, created_at_ms);
CREATE INDEX idx_subagent_tasks_status ON subagent_tasks(status, created_at_ms);
CREATE INDEX idx_subagent_tasks_source ON subagent_tasks(source_harness, source_session_id);
```

`prompt` column kept as TEXT for MVP simplicity. When the prompt exceeds 32KB, busytok-service writes it to the artifact store and sets `prompt_artifact_ref` instead of the inline `prompt` column. The sidecar RPC request carries `prompt_artifact_ref` in such cases (see section 4.3).

#### `subagent_harness_bindings`

```sql
CREATE TABLE subagent_harness_bindings (
  id TEXT PRIMARY KEY,
  subagent_id TEXT NOT NULL,
  harness TEXT NOT NULL,            -- pi (future: claude-code, codex, etc.)
  adapter_session_id TEXT,          -- e.g. pi_sess_<uuid>; sidecar-generated
  adapter_process_id TEXT,          -- sidecar pid or internal id
  is_hot INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL DEFAULT 'warm'
    CHECK(status IN ('hot', 'warm', 'closed', 'crashed')),
  created_at_ms INTEGER NOT NULL,
  last_used_at_ms INTEGER,
  closed_at_ms INTEGER,
  detail_json TEXT,
  FOREIGN KEY(subagent_id) REFERENCES subagent_logical_subagents(id)
);

-- Partial unique index: one subagent can have at most one hot binding per harness
CREATE UNIQUE INDEX idx_subagent_binding_one_hot
  ON subagent_harness_bindings(subagent_id, harness) WHERE is_hot = 1;

CREATE INDEX idx_subagent_bindings_hot ON subagent_harness_bindings(subagent_id, is_hot);
```

#### `subagent_usage_records`

```sql
CREATE TABLE subagent_usage_records (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  subagent_id TEXT NOT NULL,
  source_usage_event_id TEXT,       -- weak ref to usage_events.id, no FK; populated when task initiated by a known primary-agent session
  harness TEXT NOT NULL,
  provider TEXT,
  model TEXT,
  input_tokens INTEGER,
  output_tokens INTEGER,
  cache_read_tokens INTEGER,
  cache_write_tokens INTEGER,
  total_cost_usd REAL,
  duration_ms INTEGER,
  created_at_ms INTEGER NOT NULL,
  FOREIGN KEY(task_id) REFERENCES subagent_tasks(id),
  FOREIGN KEY(subagent_id) REFERENCES subagent_logical_subagents(id)
);

CREATE INDEX idx_subagent_usage_task ON subagent_usage_records(task_id);
```

`source_usage_event_id` is populated when the delegate request carries a `source_session_id` that can be correlated to a known `usage_events` row. It is NULL for CLI-initiated tasks or when the correlation is unavailable. The column exists for future cross-domain cost analysis but is not populated in MVP.

#### `subagent_resource_events`

Lifecycle events only — NOT high-frequency metrics.

```sql
CREATE TABLE subagent_resource_events (
  id TEXT PRIMARY KEY,
  event_type TEXT NOT NULL,
  -- sidecar_start | sidecar_stop | session_hot | session_hibernate |
  -- memory_pressure | sidecar_restart | task_timeout | rss_limit_exceeded
  target_id TEXT,
  rss_mb REAL,
  cpu_percent REAL,
  detail_json TEXT,
  created_at_ms INTEGER NOT NULL
);

CREATE INDEX idx_subagent_events_type ON subagent_resource_events(event_type, created_at_ms);
CREATE INDEX idx_subagent_events_target ON subagent_resource_events(target_id, created_at_ms);
```

### 3.3 Status State Machine

#### Logical Subagent Status (`subagent_logical_subagents.status`)

The logical subagent status reflects whether the subagent currently has an active worker:

```
cold ──(first task)──▶ hot
hot  ──(hibernate)───▶ warm
hot  ──(crash)────────▶ warm       (binding set to crashed, logical→warm for recovery)
hot  ──(delete)───────▶ deleted
warm ──(next task)────▶ hot        (new session created, context restored from memory)
warm ──(delete)───────▶ deleted
cold ──(delete)───────▶ deleted
```

**Invariants**:
- `status = 'hot'` iff there exists a `subagent_harness_bindings` row with `is_hot = 1` AND `status = 'hot'` for this subagent. The binding is authoritative for "is a worker process running."
- `status = 'warm'` means no active worker but recoverable memory exists (`subagent_memory.hot_summary IS NOT NULL`).
- `status = 'cold'` means no active worker AND no recent memory (newly created, or memory pruned after long inactivity).
- When all harness bindings for a subagent are `closed` or `crashed` with `is_hot = 0`, the logical status MUST be `warm` (if memory exists) or `cold` (if no memory).

**Crash recovery rule**: When the sidecar crashes, for each affected subagent:
1. Set `subagent_harness_bindings.is_hot = 0, status = 'crashed'`.
2. Set `subagent_logical_subagents.status` per the invariant: `'warm'` if recoverable memory exists (`subagent_memory.hot_summary IS NOT NULL`), else `'cold'`. This honors the `warm` invariant even when a task crashed before producing memory.
3. The crashed binding retains `adapter_session_id` and `detail_json` for debugging.
4. On the next task, a new binding is created and status transitions warm → hot.

#### Harness Binding Status (`subagent_harness_bindings.status`)

Represents the lifecycle of a single adapter session binding:

```
warm ──(session created)──▶ hot
hot  ──(hibernate/close)──▶ closed
hot  ──(crash)─────────────▶ crashed
closed ──(reused)──────────▶ hot     (same binding revived, rare)
crashed ──(purged)─────────▶ (row deleted by cleanup)
```

**Invariant**: At most one `is_hot = 1` row per `(subagent_id, harness)` (enforced by partial unique index).

### 3.5 Cross-Domain Relationships

- `subagent_usage_records.source_usage_event_id TEXT NULL` — optional weak reference to `usage_events.id`. No FK constraint.
- Cross-domain joins done at application layer, not enforced in SQL.
- Large artifacts (full logs, patches, traces) stored at `<data_dir>/artifacts/<subagent_id>/<task_id>/`. SQLite holds references only.

### 3.6 Artifact Store

BusytokPaths gains a new method:

```rust
pub fn artifacts_dir(&self) -> PathBuf {
    self.data_dir.join("artifacts")
}
```

`ensure_dirs_exist()` creates this directory. Subdirectories `<subagent_id>/<task_id>/` are created lazily by the subagent manager when artifacts are written.

### 3.7 Deletion Strategy

- **Soft delete** is the default: `subagent_logical_subagents.status = 'deleted'`.
- **Hard delete** (`busytok subagent delete --hard --yes`) removes task/memory/bindings/usage/events at application layer.
- No `ON DELETE CASCADE` — audit data should not be silently deleted.

---

## 4. Busytok Adapter RPC Protocol

### 4.1 Design Principles

- **Busytok owns the protocol**. Methods express Busytok concepts (logical subagent, memory, constraints, output_schema), not Pi concepts.
- **Harness-agnostic**. All fields work with future harness adapters (Claude Code, Codex, etc.).
- **Semantics reference Pi RPC** where applicable, but the protocol is not bound to Pi.
- **busytok-service initiates all requests**. Sidecar sends responses and JSON-RPC notifications; it never sends requests that require busytok-service to respond.
- **JSON-RPC 2.0 over stdio**, LF-delimited frames.

### 4.2 Methods

#### MVP Methods (required for Phase 1 MVP)

| Method | Direction | Description |
|--------|-----------|-------------|
| `adapter.initialize` | service → sidecar | Handshake: validate protocol/version/sidecar identity |
| `adapter.health` | service → sidecar | Periodic health ping |
| `adapter.shutdown` | service → sidecar | Graceful shutdown sequence |
| `session.turn_auto` | service → sidecar | Ensure hot session + execute turn in one call (MVP primary path) |
| `session.prepare_hibernate` | service → sidecar | Export session snapshot (memory delta, stats); does NOT close session |
| `session.close` | service → sidecar | Close and release a session |

#### Future Methods (designed, implemented in Step 3+)

| Method | Direction | Description |
|--------|-----------|-------------|
| `session.ensure` | service → sidecar | Ensure logical subagent has a hot session; returns `adapter_session_id` and `reused` flag |
| `session.turn` | service → sidecar | Execute a single task turn on an existing hot session |
| `session.stats` | service → sidecar | Get token/cost statistics for a session |
| `task.abort` | service → sidecar | Cancel in-flight turn |

#### Notifications (sidecar → service, one-way, no response expected)

| Method | Direction | Description |
|--------|-----------|-------------|
| `task.event` | sidecar → service | JSON-RPC 2.0 **notification** (no `id` field): discrete one-shot progress update per event. Designed for future streaming/`--no-wait` support; not consumed in MVP synchronous mode. |

#### Application Error Codes

Standard JSON-RPC 2.0 errors (-32700, -32600, -32601, -32602, -32603) plus these application-level codes:

| Code | Mnemonic | Meaning |
|------|----------|---------|
| -32001 | `SESSION_NOT_FOUND` | `adapter_session_id` does not match any active session |
| -32002 | `HOT_SESSION_LIMIT_REACHED` | Session pool is full; `data.candidate` suggests an LRU session for eviction |
| -32003 | `TASK_TIMEOUT` | Task exceeded its timeout and was aborted |
| -32004 | `SIDECAR_UNHEALTHY` | Sidecar is in a degraded state and cannot accept tasks |
| -32005 | `PROFILE_NOT_FOUND` | Requested profile is not recognized |
| -32006 | `TOOL_NOT_ALLOWED` | Requested tool is not in the profile whitelist |
| -32007 | `INVALID_OUTPUT_SCHEMA` | Output schema name/version not recognized |
| -32008 | `PROTOCOL_MISMATCH` | Adapter protocol version incompatible |

### 4.3 `session.turn_auto` — Primary MVP Path

Request:

```json
{
  "jsonrpc": "2.0",
  "id": "task_123",
  "method": "session.turn_auto",
  "params": {
    "logical_subagent_id": "f7a3b2c1-0000-0000-0000-000000000001",
    "logical_subagent_name": "auth-refresh-investigator",
    "cwd": "/Users/me/project",
    "profile": "pi/review-cheap",
    "model": "deepseek-chat",
    "tools": ["read", "grep", "git_diff"],
    "memory": {
      "hot_summary": "Previously identified token refresh logic in src/auth/token.ts.",
      "long_summary": "Long-term findings about auth module...",
      "key_files": [
        {"path": "src/auth/token.ts", "reason": "contains refreshToken logic", "last_seen_at_ms": 1710000000000, "score": 3},
        {"path": "tests/auth.test.ts", "reason": "existing test coverage", "last_seen_at_ms": 1710000000000, "score": 2}
      ],
      "decisions": ["Focus on read-only analysis first"],
      "open_questions": [
        {"question": "Does the test cover concurrent refresh scenarios?", "status": "open", "created_at_ms": 1710000000000, "last_seen_at_ms": 1710000000000}
      ]
    },
    "prompt": "Continue investigating test coverage for the auth module.",
    "prompt_artifact_ref": null,
    "context": {
      "compact_context": "You are Busytok logical subagent: auth-refresh-investigator\n\nLong-term goal: Study auth token refresh logic...\n\nKnown findings:\n- refresh logic mainly in src/auth/token.ts\n- current tests cover normal refresh path\n- concurrent refresh not yet confirmed\n\nKey files: src/auth/token.ts, tests/auth.test.ts\n\nCurrent task:\n- Continue investigating test coverage.\n\nConstraints:\n- read-only\n- do not modify files\n- output JSON",
      "budget_tokens": 4000,
      "source": "busytok-context-builder/v1"
    },
    "constraints": {
      "write_access": false,
      "timeout_ms": 180000
    },
    "output_schema": {
      "format": "json",
      "name": "review_result",
      "version": 1
    },
    "adapter_options": {}
  }
}
```

`prompt_artifact_ref` is an optional string. When set, it is a **relative path within the artifact store root** (e.g., `<subagent_id>/<task_id>/prompt.txt`). The sidecar receives the artifact store root via the `BUSYTOK_ARTIFACTS_DIR` environment variable at spawn time, resolves the full path by joining, then canonicalizes and validates it is still under `BUSYTOK_ARTIFACTS_DIR`. Absolute paths and `../` escapes are rejected by the sidecar. When null, the inline `prompt` field is used. This handles oversized prompts (>32KB) without embedding them in the JSON-RPC frame.

`adapter_options` is a harness-specific extension map. For the Pi adapter, MVP passes an empty object. Future Pi-specific options (e.g., `thinking_level`, `compaction_enabled`) go here. This field is intentionally loosely typed to accommodate per-harness options without protocol changes.

Response:

```json
{
  "jsonrpc": "2.0",
  "id": "task_123",
  "result": {
    "adapter_session_id": "pi_sess_a1b2c3d4",
    "session_reused": true,
    "status": "completed",
    "result": {
      "task_summary": "Current tests only cover normal refresh path; missing concurrent refresh and token expiry scenarios.",
      "memory_update": {
        "current_state_summary": "Review completed: test coverage gaps identified in concurrent refresh and token expiry.",
        "key_files": [
          {"path": "tests/auth.test.ts", "reason": "needs new test cases", "last_seen_at_ms": 1710000100000, "score": 1}
        ],
        "decisions": [],
        "open_questions": [
          {"question": "Should we introduce a refresh lock or singleflight?", "status": "open", "created_at_ms": 1710000100000, "last_seen_at_ms": 1710000100000}
        ]
      },
      "findings": [
        {"file": "tests/auth.test.ts", "line": 42, "message": "Missing concurrent refresh test"}
      ],
      "next_steps": ["Add concurrent refresh test", "Check refreshToken expiry fallback behavior"]
    },
    "usage": {
      "model": "deepseek-chat",
      "provider": "deepseek",
      "input_tokens": 12000,
      "output_tokens": 1800,
      "cache_read_tokens": 0,
      "cache_write_tokens": 0,
      "cost_usd": 0.0042
    },
    "artifacts": []
  }
}
```

`adapter_session_id` is generated by the sidecar as `pi_sess_<uuid_v4>`. It is unique within the sidecar process lifetime. The prefix identifies the harness; future adapters use their own prefix (e.g., `cc_sess_` for Claude Code).

**Identifier semantics**: `logical_subagent_id` in RPC requests is the **UUID primary key** from `subagent_logical_subagents.id`. busytok-service resolves the human-readable `--subagent <name>` to a UUID before sending the RPC call (see CLI name resolution in Section 7.1). `logical_subagent_name` is provided for sidecar display/logging only; the sidecar MUST use `logical_subagent_id` as the authoritative identity for session pool lookups. This separation ensures the protocol is unambiguous even if subagent renaming is added later.

### 4.4 Session Eviction Flow (Busytok-Service-Driven)

```
SubagentManager detects hot session limit exceeded
  ├─ Select LRU session (oldest last_used_at_ms among is_hot=1)
  ├─ RPC: session.prepare_hibernate(adapter_session_id)
  │
Sidecar
  ├─ Compact current Pi session
  ├─ Collect summary / key_files / open_questions / stats
  └─ Return memory snapshot
  │
SubagentManager
  ├─ Write subagent_memory (UPSERT)
  ├─ Write subagent_harness_bindings: is_hot=0, status='warm'
  ├─ Write subagent_resource_events: session_hibernate
  └─ RPC: session.close(adapter_session_id)
```

Sidecar never writes to SQLite. The sidecar-to-service channel is **notification-only** (task events); no bidirectional requests.

---

## 5. Pi Sidecar Implementation

### 5.1 Build & Distribution

- **Location**: `apps/pi-sidecar/` in Busytok monorepo.
- **Package manager**: pnpm workspace. Add `apps/pi-sidecar` to `pnpm-workspace.yaml` packages list.
- **Package name**: `@busytok/pi-sidecar` (private).
- **Dependency**: `@earendil-works/pi-coding-agent` pinned to exact version (e.g., `0.80.2`).
- **Build**: `esbuild` produces `pi-sidecar.bundle.js` (single-file ESM, Node 22 target).
- **Distribution**: Bundle + private Node.js runtime (bundled per-arch), located via `BusytokPaths`:
  ```
  <sidecar_runtime_dir>/
    pi-sidecar.bundle.js
    manifest.json
    node/darwin-arm64/node
    node/darwin-x64/node
  ```
  `BusytokPaths` provides a `sidecar_runtime_dir()` method (or `sidecar_runtime_locator`) that resolves to the correct path depending on runtime context:
  - **Packaged app**: resolves within the Tauri bundle (e.g., `apps/gui/src-tauri/binaries/sidecars/pi/`).
  - **Development mode**: resolves to `apps/pi-sidecar/dist/` and the configured Node path.
  - **Service-only (no GUI)**: configurable via settings.
  `busytok-service` does NOT directly depend on Tauri path APIs; it relies on `busytok-config` as the single source of truth for filesystem paths.
- **Spike prerequisite**: Validate that `@earendil-works/pi-coding-agent` can be esbuild-bundled. If dynamic requires prevent full bundling, fall back to vendor minimal `node_modules` in the same directory.
  - **Tool name verification**: verify that Pi SDK built-in tool names match the profile definitions (`read`, `grep`, `git_diff`). If Pi uses different names (e.g., `search` instead of `grep`), or lacks a native `git_diff` equivalent, implement custom read-only tools in the sidecar adapter layer. Do NOT assume profile tool names === Pi SDK tool names without verification.
  - **sysinfo CPU semantics**: `sysinfo` crate CPU percent requires two refresh cycles to produce meaningful values. ResourceMonitor tests must account for this; first-sample values are unreliable.
- **Node runtime**: Private, bundled per-arch; users are never required to install Node globally.
- **Codesigning**: Bundled Node binary and sidecar bundle must be included in macOS codesign and notarization.
- **Workspace changes**: Add `sysinfo` to `[workspace.dependencies]` in root `Cargo.toml`.

### 5.2 Session Pool

```
SessionPool {
  max_hot: N (default 3, configurable)
  sessions: Map<adapter_session_id, PiSessionHandle>
  subagent_map: Map<logical_subagent_id, adapter_session_id>
  lru: ordered list of adapter_session_id (most-recently-used first)

  ensure(logical_subagent_id, opts) → adapter_session_id:
    1. Hit subagent_map → bump LRU → return existing session
    2. Miss + not full → createSession() via Pi SDK → add to pool
    3. Miss + full → return error {code: "HOT_SESSION_LIMIT_REACHED", candidate: lru_last}
       busytok-service then drives eviction via prepare_hibernate + close.
}
```

### 5.3 Tool Whitelist

Double-layered enforcement:

1. **busytok-service** resolves profile → allowed tools list and sends it in the RPC request.
2. **Sidecar** registers only allowed tools with Pi SDK, never exposes blocked tools.

Profiles:

| Profile | Tools | Write Access |
|---------|-------|:---:|
| `pi/search-cheap` | `read`, `grep` | No |
| `pi/review-cheap` | `read`, `grep`, `git_diff` | No |
| `pi/plan-cheap` | `read`, `grep`, `git_diff` | No |
| `pi/patch-small` | `read`, `grep`, `edit` | Yes **[DEFERRED — not in MVP]** |

### 5.4 Sidecar Process Lifecycle

```
busytok-subagent::sidecar::PiSidecarSupervisor

  Start:
    - On-demand: first Pi task arrival triggers spawn
    - spawn(bundled_node, [pi-sidecar.bundle.js])
    - Wait for adapter.initialize response
    - Verify protocol version, sidecar version, Pi version

  Health:
    - Periodic adapter.health ping (every 30s)
    - On failure: mark sidecar unhealthy, attempt restart

  Idle Exit:
    - No active tasks for pi_idle_exit_seconds (default 300)
    - → hibernate all sessions → adapter.shutdown → exit

  Crash Recovery:
    - Detect subprocess exit (non-zero, or unexpected)
    - For each in-flight task (status = 'running' on the crashed sidecar):
      - Mark task status = 'failed', error = 'SIDECAR_CRASHED'
      - Write resource_event: sidecar_crash, detail_json includes task_id
    - Release all hot harness bindings: is_hot = 0, status = 'crashed'
    - Exponential backoff restart: 1s → 2s → 4s → 8s (max 3 attempts per 5 min)
    - After restart: all sessions cold; next task restores from SQLite memory
    - Crashed tasks are NOT auto-retried by default
    - busytok-service MUST NOT crash when sidecar dies

  Graceful Shutdown:
    - SIGTERM → prepare_hibernate all hot sessions → adapter.shutdown → wait 10s → SIGKILL

  Resource Monitoring:
    - Collect sidecar RSS/CPU via sysinfo crate (sampled every ~30s for health decisions)
    - Emit resource events at lifecycle boundaries only (not a metrics time-series table)

  API Key Management:
    - API keys for model providers are loaded from busytok-config settings (a new `providers` section
      mapping provider name to env-var or credential reference).
    - busytok-service passes credentials to the sidecar via environment variables set before spawn
      (e.g., `DEEPSEEK_API_KEY`, `QWEN_API_KEY`), following the same pattern Pi's env-api-keys.ts uses.
    - The sidecar's Pi SDK reads keys from the environment; busytok-service never sends keys in RPC params.
    - Provider credential configuration is deferred to implementation — the settings schema is scoped
      during Step 1. For MVP, using the same env vars that Pi natively supports is sufficient.
    - **Credential rotation**: environment variables are set at spawn time only. If a provider key
      changes while the sidecar is running, the sidecar will not see the new value. On settings change
      that affects provider credentials, busytok-service marks the sidecar as `restart_required`. The
      next health cycle or task request triggers a graceful sidecar restart (hibernate all → shutdown
      → restart with new env). MVP fallback: document that key changes require restarting busytok-service.
```

---

## 6. Context Builder & Memory

### 6.1 Context Builder

**Input** (from SQLite):
- `subagent_logical_subagents`: name, intent, default_profile
- `subagent_memory`: hot_summary, long_summary, key_files, decisions, open_questions
- `subagent_tasks`: last N completed tasks (default N=5), prompt + result_summary only
- Current task: prompt, profile, constraints
- Repo metadata: repo_path, branch

**Output**: A `CompactContext` struct containing:
- `compact_context`: the final prompt-ready context string, built by busytok-service (ContextBuilder), passed to the sidecar via the `context.compact_context` RPC field.
- `budget_tokens`: the token budget that was applied.

The sidecar consumes `context.compact_context` as the primary context source and injects it into the Pi session system prompt. The `memory` field in the RPC request carries the structured raw data for debugging and future direct-sidecar consumption, but the sidecar MUST use `compact_context` as the authoritative context.

**Responsibility boundary**:
- **busytok-service / busytok-subagent**: responsible for context trimming, budget control, and producing `compact_context`.
- **Pi sidecar**: responsible for placing `compact_context` into the Pi session / system prompt; does NOT perform its own context assembly or trimming.

**Budget**: Default 4000 tokens, per-profile configurable. Hard max 8000 tokens.

**Trim priority** (when over budget, trim in order):
1. Recent task summaries: N=5 → N=3 → N=1
2. Attempts: keep last 3
3. Key files: 20 → 10
4. Open questions: 10 → 5
5. Long summary: truncate to max chars
6. Hot summary: preserve as much as possible (it represents current state)
7. Current task prompt: **never trimmed** unless it exceeds hard limit → route to artifact store

**What is NEVER injected**:
- Full task history (summary lines only)
- Full artifacts (reference paths only)
- Other subagents' context
- Raw log output or full trace files

### 6.2 Memory Update

After each task completes, `MemoryUpdater`:

1. Read current `subagent_memory`.
2. Merge:
   - `key_files` = normalize paths + merge dedupe + update `last_seen_at_ms` and `score`
   - `open_questions` = normalize + merge dedupe (exact match on lowercase question) + mark resolved/closed
   - `decisions` = merge dedupe
   - `attempts` = append (last 10)
   - `hot_summary` = `result.memory_update.current_state_summary` (NOT `task_summary`)
3. Check compaction triggers (any of):
   - >= 5 tasks since last compaction
   - `hot_summary` + `long_summary` + recent summaries > 70% of context budget
   - Hibernate is about to happen
4. If triggered, rebuild `long_summary`.

**MVP compaction algorithm** (rule-based, no LLM):

```
new_long_summary =
  // Start with stable conclusions from old long_summary
  (old.long_summary, truncated to 2000 chars)
  + "\n\nRecent findings:\n"
  // Append recent task summaries
  + (concatenate last 5 task result_summary lines, truncated to 1000 chars)

Apply per-category caps:
  - Total long_summary: max 3000 chars
  - decisions: max 20 entries
  - key_files: max 20 entries
  - open_questions: max 10 (unresolved only)
  - attempts: last 10 entries
```

This is a concatenation + truncation algorithm, not LLM-based summarization. LLM-based compaction is a future enhancement (deferred item 10).

5. Write back `subagent_memory` UPSERT.
6. Update `last_compacted_at_ms` and `last_compacted_task_id` if compaction was performed.

### 6.3 Normalization

- **key_files**: repo-relative path, normalize separators, strip `./` prefix, macOS case-insensitive dedup.
- **open_questions**: trim, dedupe by lowercase exact match, preserve original casing for display. Support `status: open | resolved`.

### 6.4 Concurrency Model

- **Same logical subagent**: tasks are **serialized** (FIFO queue per subagent). A new `delegate` on an already-running subagent is queued; the caller waits (or times out) until the prior task completes.
- **Different logical subagents**: tasks run concurrently up to `max_hot_sessions` (default 3). Each occupies one hot session slot.
- **Queue full**: when the per-subagent queue depth exceeds a small limit (default 3 queued tasks per subagent), new `delegate` requests are rejected with a "queue full" error.
- **Global task cap**: `task_queue_max` (default 50) limits total queued+running tasks across all subagents. Exceeding this returns an error.

Implemented in `busytok-subagent::ConcurrencyController`.

---

## 7. CLI Design

### 7.1 Commands

#### `busytok delegate`

```bash
busytok delegate \
  --subagent <name> \           # Logical subagent name (required)
  --intent <intent> \           # Task intent tag
  --profile <profile> \         # pi/search-cheap | pi/review-cheap | pi/plan-cheap
  --cwd <path> \                # Working directory (default: current)
  --model <model> \             # Override profile default model
  --wait \                      # Wait for completion (default, MVP only mode)
  --timeout <seconds> \         # Override profile default timeout
  --output json|text \          # Output format (default: text)
  "<task prompt>"
```

**Subagent name resolution**: name is resolved by:
1. Derive `repo_hash` from `cwd` using `busytok-domain::derive_project_hash(repo_path)`.
2. Set `project_id = repo_hash` (MVP simplification; future versions may decouple these).
3. Query: `SELECT * FROM subagent_logical_subagents WHERE name = ? AND project_id = ? AND repo_hash = ? AND status != 'deleted'`.
4. The partial unique index `idx_subagent_unique_active_name(project_id, repo_hash, name)` guarantees at most one active match.
5. Full UUID via `--id <uuid>` bypasses name resolution entirely.

`project_id = repo_hash` in MVP means the (project_id, repo_hash, name) unique constraint collapses to (repo_hash, name) in practice. This simplification is documented so future versions that decouple project_id from repo_hash can extend the resolution logic without breaking existing data.

**--no-wait**: Designed but **[DEFERRED to Phase 2]**. MVP only supports synchronous `--wait`.

**--output json** produces a stable canonical result:

```json
{
  "task_id": "...",
  "subagent_id": "...",
  "subagent_name": "...",
  "adapter": "pi",
  "adapter_session_id": "...",
  "session_reused": true,
  "status": "completed",
  "profile": "pi/review-cheap",
  "model": "...",
  "output_schema_name": "review_result",
  "output_schema_version": 1,
  "summary": "...",
  "result": {},
  "usage": {
    "model": "...",
    "provider": "...",
    "input_tokens": 12000,
    "output_tokens": 1800,
    "cache_read_tokens": 0,
    "cache_write_tokens": 0,
    "cost_usd": 0.0042
  },
  "artifacts": []
}
```

`result` is the canonical structured output. `summary`/`findings`/`key_files`/`open_questions` are retained as top-level convenience fields but `result` is authoritative.

**Profile-specific timeouts**:

| Profile | Default Timeout |
|---------|---------------:|
| `pi/search-cheap` | 120s |
| `pi/review-cheap` | 180s |
| `pi/plan-cheap` | 300s |
| Hard max (any profile) | 600s |

#### `busytok subagent list`

```bash
busytok subagent list [--status hot|warm|cold] [--project <id>] [--include-deleted]
```

#### `busytok subagent show`

```bash
busytok subagent show <name> [--cwd <path>]
busytok subagent show --id <uuid>
```

Displays: id, name, status, repo, branch, profile, hot summary, key files, open questions, recent 5 task summaries, usage rollup.

#### `busytok subagent tasks`

```bash
busytok subagent tasks <name> [--limit 20] [--status completed|failed|running]
```

#### `busytok subagent hibernate`

```bash
busytok subagent hibernate <name>
```

Releases Pi hot session, writes memory to SQLite, status hot → warm. Does NOT delete DB data.

#### `busytok subagent delete`

```bash
busytok subagent delete <name>                 # Soft delete (status = 'deleted')
busytok subagent delete <name> --hard --yes    # Physical delete
```

`--hard` requires `--yes` for confirmation protection.

#### `busytok doctor`

```bash
busytok doctor
```

Checks:
- busytok-service running
- SQLite readable/writable + schema version = 2
- Pi sidecar launchable (bundled node)
- Bundled Node architecture matches current machine
- Sidecar bundle manifest readable
- Protocol version matches
- Default model configuration valid
- Pi runtime installed
- Artifact store writable
- Resource policy valid
- Subagents unused > 30 days (warning)

### 7.2 Communication Path

```
CLI → busytok-control IPC client → busytok-service → busytok-runtime → SubagentManager
```

CLI never directly accesses SQLite or manages the Pi sidecar process.

**Subagent commands do NOT support offline mode in MVP.** If busytok-service is not running, subagent CLI commands return a clear error instructing the user to start the service. This is a deliberate constraint: subagent functionality requires the service lifecycle (sidecar management, concurrency control, memory persistence). The existing `busytok --offline` flag for scan operations does not apply to subagent commands.

### 7.3 Control Protocol Methods

New methods follow the existing `resource.action` naming convention and the existing codebase pattern:

- **`busytok-protocol`**: defines method names and request/response DTOs in `method_manifest()`.
- **`busytok-control`**: contains the IPC transport layer (`ControlServer`, `ControlClient`) and the `ControlDispatcher` which routes incoming method names to handler functions. This crate is responsible for method routing; it does NOT contain subagent business logic.
- **`busytok-runtime`**: implements the `RuntimeControl` trait (or equivalent handler interface) with the actual business logic. `SubagentManager` methods are called from these handler implementations.
- **`apps/service`**: wires control server → runtime handlers at boot time.

This follows the existing pattern in the codebase (cf. `crates/busytok-control/src/dispatch.rs`).

| Method | Description | Implemented In |
|--------|-------------|----------------|
| `subagent.delegate` | Create or continue a logical subagent and execute a task | busytok-runtime (RuntimeControl) |
| `subagent.list` | List logical subagents, optionally filtered by status/project | busytok-runtime (RuntimeControl) |
| `subagent.show` | Get detailed view of a single logical subagent | busytok-runtime (RuntimeControl) |
| `subagent.tasks` | List task history for a subagent | busytok-runtime (RuntimeControl) |
| `subagent.hibernate` | Release hot session, persist memory | busytok-runtime (RuntimeControl) |
| `subagent.delete` | Soft or hard delete a subagent | busytok-runtime (RuntimeControl) |
| `doctor.check` | Extended with subagent-related health checks | busytok-runtime (RuntimeControl) |

These follow the existing `resource.action` naming convention.

---

## 8. Resource Management

### 8.1 Collection

`ResourceMonitor` in `busytok-subagent` periodically collects (via `sysinfo` crate):

- busytok-service RSS
- Pi sidecar RSS + CPU
- Hot session count
- Queued / running task count
- System available memory (macOS)

### 8.2 Policy Defaults

Sidecar lifecycle settings live in `[subagent.pi_sidecar]` (see Section 10.1 for the full TOML schema). Key defaults:

| Setting | Default | Location |
|---------|---------|----------|
| `max_hot_sessions` | 3 | `[subagent.pi_sidecar]` |
| `idle_exit_seconds` | 300 | `[subagent.pi_sidecar]` |
| `hibernate_after_seconds` | 600 | `[subagent.pi_sidecar]` |
| `task_timeout_seconds` | 300 | `[subagent.pi_sidecar]` (global fallback; profile overrides) |
| `task_queue_max` | 50 | `[subagent.pi_sidecar]` |
| `memory_soft_limit_mb` | 800 | `[subagent.pi_sidecar]` |
| `memory_hard_limit_mb` | 1200 | `[subagent.pi_sidecar]` |

System-level resource thresholds live in `[subagent.resource_policy]`:

```toml
[subagent.resource_policy]
memory_pressure_free_mb = 2048    # System free memory threshold
monitor_interval_seconds = 30     # Resource sampling interval
```

### 8.3 Pressure Response

1. Hibernate least-recently-used hot session.
2. Pause new task execution (queue only).
3. If sidecar RSS exceeds soft limit → request graceful restart.
4. Before restart: prepare_hibernate all hot sessions → write memory → restart.
5. If graceful restart fails → force kill. Completed task state in SQLite is never lost.

---

## 9. Security & Isolation

### 9.1 MVP: Read-Only Profiles Only

`pi/search-cheap`, `pi/review-cheap`, `pi/plan-cheap` are **read-only**:
- No write_file
- No shell/bash execution
- Only `read`, `grep`, `git_diff` tools

Enforced at two layers:
1. **busytok-service**: profile → allowed tools list sent in RPC.
2. **sidecar**: Pi SDK tool registration only includes allowed tools; blocked tools never exposed to the model.

Prompt-level "don't write files" is NOT sufficient as the sole enforcement mechanism.

### 9.2 Write Mode [DEFERRED]

`pi/patch-small` is designed but NOT implemented in MVP. When implemented:

- Original repo is never modified directly.
- Task creates a git worktree at `.busytok/worktrees/<task_id>`.
- Changes made in the worktree.
- Results returned as patch/diff.
- Worktree cleaned up after task completion.

---

## 10. Configuration

### 10.1 Settings Structure

Extensions to existing `busytok-config` settings. The existing `BusytokSettings` struct gains a `subagent: Option<SubagentSettings>` field. `SubagentSettings` is a separate struct following the existing pattern (cf. `PrivacySettings`, `DiscoverySettings`), containing nested sub-structs for `PiSidecarConfig`, `ContextConfig`, `ResourcePolicyConfig`, `ModelsConfig`, and a `HashMap<String, ProfileConfig>` for profiles.

TOML format (valid syntax):

```toml
[subagent]
enabled = true

[subagent.pi_sidecar]
enabled = true
node_runtime = "bundled"           # "bundled" | "system"
system_node_path = ""              # used only when node_runtime = "system"
max_hot_sessions = 3
idle_exit_seconds = 300
hibernate_after_seconds = 600
task_timeout_seconds = 300
memory_soft_limit_mb = 800
memory_hard_limit_mb = 1200
task_queue_max = 50

[subagent.context]
default_budget_tokens = 4000
max_budget_tokens = 8000
recent_tasks_limit = 5
compaction_tasks_threshold = 5
compaction_budget_ratio = 0.7

[subagent.resource_policy]
memory_pressure_free_mb = 2048
monitor_interval_seconds = 30

[subagent.models]
default_cheap_model = "deepseek-chat"
default_review_model = "qwen-coder"
default_reasoning_model = "deepseek-reasoner"
default_coder_model = "qwen-coder"   # for future pi/patch-small profile

[subagent.profiles."pi/search-cheap"]
write_access = false
tools = ["read", "grep"]
model = "deepseek-chat"
context_budget_tokens = 3000
timeout_seconds = 120

[subagent.profiles."pi/review-cheap"]
write_access = false
tools = ["read", "grep", "git_diff"]
model = "qwen-coder"
context_budget_tokens = 5000
timeout_seconds = 180

[subagent.profiles."pi/plan-cheap"]
write_access = false
tools = ["read", "grep", "git_diff"]
model = "deepseek-reasoner"
context_budget_tokens = 6000
timeout_seconds = 300

# [DEFERRED — not in MVP]
# [subagent.profiles."pi/patch-small"]
# write_access = true
# tools = ["read", "grep", "edit"]
# model = "qwen-coder"
# isolation = "git_worktree"
```

Model references above are literal strings, not variable interpolation. The `models.*` values serve as defaults referenced by profile definitions at settings-load time.

---

## 11. MVP Scope

### Must Implement

1. `busytok-subagent` crate with Manager, Router, ContextBuilder, MemoryUpdater, ConcurrencyController, ResourceMonitor.
2. SQLite schema: `0002_subagent.sql` (6 tables + indices + CHECK constraints, `_ms` timestamp suffix).
3. `apps/pi-sidecar` TypeScript package in pnpm workspace.
4. Pi sidecar JSON-RPC server with MVP methods: `adapter.initialize`, `adapter.health`, `adapter.shutdown`, `session.turn_auto`, `session.prepare_hibernate`, `session.close`.
5. Pi SDK bundle spike (validate esbuild + createAgentSession works).
6. Bundled Node.js runtime in Tauri binaries.
7. Pi sidecar subprocess management (spawn, health, crash recovery, graceful shutdown, idle exit).
8. Hot session pool with LRU tracking and busytok-service-driven eviction.
9. `busytok delegate --wait` CLI command (sync only).
10. `busytok subagent list/show/tasks/hibernate/delete` CLI commands.
11. `busytok doctor` CLI command (extended with subagent checks).
12. Read-only profiles: `pi/search-cheap`, `pi/review-cheap`, `pi/plan-cheap`.
13. Basic usage recording per task.
14. Rule-based MVP compaction (concatenation + truncation, no LLM).
15. Resource event recording at lifecycle boundaries.
16. Context builder with budget control and trim priority.
17. Memory updater with merge rules and compaction triggers.
18. Protocol version validation on sidecar handshake.
19. Per-subagent task serialization (concurrency control).
20. Artifact store directory in BusytokPaths.
21. Control protocol methods: `subagent.delegate`, `subagent.list`, `subagent.show`, `subagent.tasks`, `subagent.hibernate`, `subagent.delete`.
22. `sysinfo` crate added to workspace dependencies.
23. `apps/pi-sidecar` added to pnpm workspace config.
24. `BusytokPaths::artifacts_dir()` and `ensure_dirs_exist()` update.

### Deferred

1. Claude Code / Codex / OpenCode / Aider / Goose adapters.
2. `busytok delegate --no-wait` and `busytok task show/wait/cancel`.
3. `pi/patch-small` write-mode profile.
4. Complex sandboxing beyond tool whitelist.
5. Cloud sync / team multi-user.
6. Vector database for memory.
7. Automatic task classification and model selection.
8. Node SEA (Single Executable Application) packaging.
9. `subagent_memory_snapshots` multi-version history.
10. LLM-based long summary compaction.
11. `session.ensure`, `session.turn`, `session.stats`, `task.abort` protocol methods (designed, implemented in Step 3+).
12. `task.event` notification consumption (designed, used when `--no-wait` is implemented).

---

## 12. Acceptance Criteria

### 12.1 Functional

**Case 1 — Same logical subagent, consecutive tasks:**
```bash
busytok delegate --subagent auth-test --profile pi/search-cheap "Study auth module"
busytok delegate --subagent auth-test --profile pi/review-cheap "Continue: check test coverage"
```
Expected: second task sees first task's summary and key_files. Same `adapter_session_id` reused if hot session still alive. No duplicate logical subagent created.

**Case 2 — Hibernate then restore:**
```bash
busytok delegate --subagent auth-test --profile pi/search-cheap "Study auth module"
busytok subagent hibernate auth-test
busytok delegate --subagent auth-test --profile pi/review-cheap "Continue previous work"
```
Expected: Pi session released. busytok-service restores memory from SQLite. New Pi session created and continues with context from DB.

**Case 3 — Hot session limit:**
```toml
max_hot_sessions = 2
```
```bash
busytok delegate --subagent a ...
busytok delegate --subagent b ...
busytok delegate --subagent c ...
```
Expected: max 2 hot sessions. LRU subagent hibernated. All DB state intact.

**Case 4 — Sidecar crash recovery:**
Execute task, then `kill -9` the Pi node subprocess.
Expected: busytok-service does not crash. Next delegate auto-restarts sidecar. Memory restored from SQLite. Task history preserved.

**Case 5 — Concurrent tasks on same subagent are serialized:**
```bash
# Terminal 1
busytok delegate --subagent X --profile pi/search-cheap "Long analysis task" &
# Terminal 2 (immediately after)
busytok delegate --subagent X --profile pi/review-cheap "Quick follow-up"
```
Expected: Terminal 2's task is queued and waits for Terminal 1's task to complete. Both tasks record results in DB.

**Case 6 — Concurrent tasks on different subagents run in parallel:**
```bash
busytok delegate --subagent X --profile pi/search-cheap "Task for X" &
busytok delegate --subagent Y --profile pi/review-cheap "Task for Y" &
```
Expected: both tasks run concurrently (up to max_hot_sessions limit).

### 12.2 Resource

- Idle busytok-service RSS < 50MB (Pi sidecar not running).
- 100 idle logical subagents: busytok-service RSS does not grow linearly with subagent count (they are DB rows, not processes).
- Pi sidecar: exactly 1 process when active.
- `max_hot_sessions` enforced at sidecar level.
- After hibernate, sidecar hot session count decreases.
- Sidecar exits after idle TTL with no active tasks.

---

## 13. Implementation Sequence

### Step 1 — SQLite Schema + CLI Skeleton
- Migration `0002_subagent.sql`.
- `subagent_*` repository methods in `busytok-store`.
- `BusytokPaths::artifacts_dir()`.
- `busytok-subagent` crate scaffold (Manager, models, error, ConcurrencyController).
- New control protocol methods registered in `busytok-protocol`; handlers implemented in `busytok-runtime` (via `RuntimeControl` trait, following the existing dispatch pattern in `busytok-control`).
- CLI commands wired through control protocol (mock responses, no Pi).
- Update `baseline_single_migration()` test.
- `sysinfo` added to workspace deps; `apps/pi-sidecar` added to pnpm workspace.

### Step 2 — Pi Sidecar Minimum JSON-RPC
- `apps/pi-sidecar` TypeScript package.
- Pi SDK bundle spike (esbuild + createAgentSession).
- JSON-RPC server: `adapter.initialize`, `adapter.health`, `adapter.shutdown`, `session.turn_auto`, `session.prepare_hibernate`, `session.close`.
- Pi sidecar subprocess management in `busytok-subagent::sidecar::PiSidecarSupervisor`.
- Sidecar supervisor: spawn, health ping, crash detection/restart, graceful shutdown, idle exit.

### Step 3 — Hot Session Pool
- Sidecar-side session pool with LRU tracking.
- busytok-service-driven eviction flow (end-to-end: select LRU → `prepare_hibernate` → write memory → `close`).
- Protocol methods `session.ensure` and `session.turn` (advanced path; `session.turn_auto` covers the common case).
- Idle TTL + auto-exit.

### Step 4 — Context Builder + Memory Update
- ContextBuilder with budget control and trim priority.
- MemoryUpdater with merge rules and rule-based compaction.
- End-to-end: delegate → build context → sidecar turn → update memory.

### Step 5 — Resource Monitoring + Validation
- ResourceMonitor (sysinfo-based RSS/CPU collection).
- Resource event recording.
- Doctor command (extended).
- 100-subagent stress test script.
- Sidecar crash recovery test.
- Acceptance test suite.

---

## 14. Deliverables

1. `crates/busytok-subagent/` — Rust crate.
2. `apps/pi-sidecar/` — TypeScript sidecar package.
3. `crates/busytok-store/migrations/0002_subagent.sql` — Schema migration.
4. CLI commands: `delegate`, `subagent {list,show,tasks,hibernate,delete}`, `doctor`.
5. JSON-RPC protocol documentation (this spec, section 4).
6. Default profiles configuration (in `busytok-config` settings).
7. Updated `BusytokPaths` with `artifacts_dir()`.
8. Stress test script: 100 subagents.
9. README updates: install, start, debug, acceptance.
10. Engineering design doc (this file).
11. Future harness adapter guide: how Claude Code / Codex / OpenCode adapters plug into the same protocol.

---

## 15. Open Design Questions (Resolved)

| Question | Resolution |
|----------|------------|
| busytokd: extend service or new daemon? | Extend busytok-service (no separate binary) |
| Pi sidecar: RPC CLI or SDK? | Pi SDK via custom TypeScript sidecar |
| Pi sidecar distribution? | Bundled private Node + esbuild JS bundle in DMG (A+) |
| New tables: same DB or separate? | Same `busytok.db`, migration 0002, `subagent_` prefix |
| JSON-RPC: Pi-native or Busytok-owned? | Busytok-owned harness-agnostic protocol (C) |
| Build integration? | pnpm workspace, `@busytok/pi-sidecar` |
| Module organization? | New `busytok-subagent` crate |
| session.ensure + session.turn: merge or separate? | Separate in protocol; `session.turn_auto` as convenience |
| Eviction: sidecar-initiated or busytok-service-initiated? | busytok-service-driven; sidecar never writes DB |
| subagent_memory: 1:1 or multi-version? | MVP 1:1; snapshots deferred |
| result_json: schema version? | Yes, `output_schema_name` + `output_schema_version` |
| Context budget: fixed or configurable? | Configurable per-profile, default 4000 tokens |
| Compaction trigger: task count only? | 3 conditions: task count, budget ratio, pre-hibernate |
| Compaction algorithm (MVP)? | Rule-based concat+truncation (no LLM) |
| --timeout default: 300s fixed? | Profile-specific (120/180/300s) |
| --no-wait in MVP? | Deferred to Phase 2 |
| Soft delete or hard delete? | Soft delete default; hard delete with --hard --yes |
| Subagent name scope? | Unique within repo_hash scope; resolved via derive_project_hash |
| Timestamp column naming? | Use `_ms` suffix (match existing convention) |
| Concurrency on same subagent? | Serialized (FIFO per subagent); different subagents run concurrently |
| API key management? | Env vars set by busytok-service before sidecar spawn; not in RPC params |
| Tauri resource path? | `apps/gui/src-tauri/binaries/sidecars/pi/` |
