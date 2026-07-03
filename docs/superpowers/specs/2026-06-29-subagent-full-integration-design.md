# Subagent Full Integration Design

**Date:** 2026-06-29
**Status:** Approved (brainstorming complete)
**Scope:** Close all six gaps between "subagent foundation shipped (v0.0.8)" and "user downloads, configures API key, gets full subagent functionality"
**Decomposition:** One master spec (this document) + 5 implementation plans (one per phase)

---

## 1. Vision

User downloads Busytok DMG, installs, opens the GUI, configures model provider + API key in the Providers page, and AI agents (Claude Code / Codex) can immediately delegate tasks via `busytok delegate` CLI to cheap models (DeepSeek, Qwen, OpenRouter, etc.) through the Pi sidecar.

Subagent delegation is **CLI-only, for AI agents** — the GUI is configuration + monitoring, not delegation.

---

## 2. Architecture

### 2.1 Core Principles

1. Config stores non-sensitive provider metadata + explicit profile bindings; secrets only in OS keychain.
2. Provider resolution done in `busytok-service`; sidecar does no guesswork provider selection.
3. Credential injection is explicitly linked to session reuse strategy, not relying on long-lived `process.env`.
4. Rust manages provider worker processes; Node manages the session pool.
5. Usage normalization and billing are unified in Rust.

### 2.2 Component Diagram

```
┌─ GUI (React) ──────────────────────────────────────┐
│  ┌─ Providers 页 ─┐  ┌─ Subagents 页 ─┐            │
│  │ CRUD 供应商     │  │ 只读监控        │            │
│  │ API key 输入    │  │ 压力/sidecar健康 │            │
│  │ Profile 绑定    │  │ 任务历史        │            │
│  └───────┬────────┘  └───────┬────────┘            │
│          │ invoke_busytok     │ invoke_busytok       │
├──────────┼────────────────────┼────────────────────┤
│          ▼                    ▼                     │
│   ┌─ busytok-gui (Tauri host) ──────────────┐       │
│   │   transparent RPC forwarding             │       │
│   └──────────────┬──────────────────────────┘       │
└──────────────────┼─────────────────────────────────┘
                   ▼
┌─ busytok-service (daemon) ──────────────────────────┐
│                                                       │
│  RPC: provider.* / profile.* / subagent.*             │
│                 ↓                                     │
│  ┌─ Credential Store ───────────────────────┐        │
│  │  keyring-rs → macOS Keychain              │        │
│  │  provider config → settings.toml          │        │
│  └──────────────┬───────────────────────────┘        │
│                 ↓ (delegate 时)                        │
│  ┌─ PiSidecarSupervisor ───────────────────┐         │
│  │  per-provider worker management           │         │
│  │  keyring.get() → env injection at spawn   │         │
│  └──────────────┬───────────────────────────┘        │
│                 ▼                                     │
│  ┌─ Node sidecar workers ──────────────────┐         │
│  │  one process per active provider          │         │
│  │  Pi SDK → model API → response + usage    │         │
│  └──────────────────────────────────────────┘         │
└───────────────────────────────────────────────────────┘
```

### 2.3 Credential Flow

```
Write: Provider page → invoke_busytok("provider.create", {config, api_key})
       → service → settings.toml writes metadata + keyring.set("com.busytok.providers", id, key)

Delegate: CLI → invoke_busytok("subagent.delegate", {name, profile, prompt})
       → service resolves profile → provider → keyring.get()
       → ensure worker(provider_id) running (stale/auth-fail → respawn)
       → spawn: node pi-sidecar.bundle.js
         env: {api_key_env_name: key, base_url_env_name: base_url}
       → JSON-RPC: session.turn_auto (no credential fields)

Logging: key never persists to disk, never enters logs, never leaves the local machine.
         Key travels only via local controlled IPC (Unix socket) from GUI to service during provider setup.
```

### 2.4 CONTRIBUTING.md Invariant Update

Old:
> "never stores credentials"

New:
> "Stores provider API keys exclusively in the OS keychain (macOS Keychain / Windows Credential Manager) via keyring-rs. Keys are never written to config files, never logged, never transmitted over network. Keys are injected into the sidecar subprocess via environment variables at spawn time only."

---

## 3. Data Models

### 3.1 Provider

```
Provider (settings.toml — non-sensitive)
  id:                     String         ← immutable primary key
  name:                   String         ← display name, editable
  provider_kind:          String         ← "openai_compatible" (determines protocol adaptation)
  base_url:               String         ← editable
  api_key_env_name:       String         ← injection name only, not routing (e.g. "DEEPSEEK_API_KEY")
  base_url_env_name:      Option<String> ← injection name, defaults to api_key_env_name's base counterpart
  models:                 Vec<String>    ← whitelist Busytok allows for this provider
  enabled:                bool           ← can be disabled without losing key

API Key (Keychain)
  service:                "com.busytok.providers"   ← stable namespace
  account:                provider_id                ← not display name
  secret:                 "sk-..."
```

### 3.2 Profile

```
Profile (settings.toml)
  id:                     String           ← immutable for built-in; user-chosen for custom
  provider_id:            Option<String>   ← explicit binding; None = unbound (post-upgrade state)
  model:                  String           ← must ∈ selected provider's models
  tools:                  Vec<String>      ← read-only in MVP UI
  context_budget_tokens:  u32              ← read-only in MVP UI
  timeout_seconds:        u32              ← read-only in MVP UI
  is_builtin:             bool             ← read-only; built-in cannot be deleted
```

### 3.3 Keychain Lifecycle Rules

| Operation | settings.toml | Keychain | Rationale |
|---|---|---|---|
| create provider | write metadata | write secret | initial binding |
| rename provider (name) | update name only | **no change** (account = id) | id is immutable |
| update API key | no change | overwrite secret | secret-only change |
| update base_url | update | no change | non-secret |
| disable provider | `enabled: false` | **retain** | user may re-enable |
| delete provider | delete | **delete** (sync) | prevent orphan secrets |
| provider/key change | write | write | worker marked stale → rebuild on next delegate |

### 3.4 Constraints

- `provider.id` is immutable after creation.
- `provider.models` is a whitelist: service must validate `profile.model ∈ provider.models` before delegate.
- `provider_kind` determines protocol adaptation behavior; `api_key_env_name` / `base_url_env_name` are injection names only and do not participate in routing decisions.
- MVP `provider_kind` enum: single variant `openai_compatible`. Extensible to `anthropic`, `google`, etc. in future phases.
- Built-in profiles are canonicalized by service on load: only fill missing entries, never overwrite user-modified provider/model bindings.
- On upgrade from pre-Phase 1 configs (profiles have no `provider_id`): canonicalization treats them as present but `provider_id = None` (invalid). UI shows stale/unbound state until user binds. Built-in profiles are NOT auto-assigned to any provider.
- `providers` lives as a new top-level field in `BusytokSettings`: `providers: Vec<ProviderConfig>`. In TOML, serialized as array-of-tables `[[providers]]` (one `[[providers]]` block per provider). Profiles remain under `[subagent.profiles]`.
- Price catalog: user-added provider models not in the static `PriceCatalog` → `cost_status = Unavailable` (same tri-state as existing pipeline). `cost_usd = None`.

### 3.5 Config Migration (backward-compat)

When upgrading from v0.0.8 (no Provider concept):
1. `providers` field absent → initialized to empty `Vec`.
2. Existing built-in profiles have no `provider_id` → canonicalization preserves them with `provider_id = None`.
3. User must create a Provider in the GUI, then bind each profile to it.
4. Delegate on an unbound profile returns validation error: "profile not bound to a provider."
5. No silent auto-assignment — user explicitly chooses the binding.

---

## 4. Phase Breakdown

### Phase 1: Credential Foundation + Provider Page

**Gap closed:** API key management (Gap 2)

**Deliverables:**
- `keyring-rs` integration in `busytok-service` (sole credential owner)
- Provider config in `settings.toml` (metadata) + Keychain (secrets)
- New RPC methods: `provider.create / list / update / delete / test_connection`
- Key management is merged into the CRUD surface (no separate `set_key` / `get_key_status` RPCs): `provider.create` / `provider.update` accept an optional `api_key` field (written to keyring, never persisted to `settings.toml`); `provider.list`'s `ProviderDto` exposes `has_api_key: bool` ("set" / "not set" — key never returned in plaintext); `provider.test_connection` surfaces "invalid" by probing the upstream API. Rationale: fewer RPCs with the same capability; key status is read-only metadata best served alongside the provider object; a separate `set_key` would add a round-trip for the common "create provider + set key" flow.
- `test_connection`: lightweight Rust-side HTTPS probe (`GET /v1/models` or `POST /v1/chat/completions` with 1-token prompt). Key read from keyring in-memory, used for the probe, never persisted/logged. This is a service health probe, not user-traffic proxying — does not violate the "never proxies traffic" invariant (which is about relaying agent traffic).
- GUI: new "Providers" sidebar page with CRUD + API key input + connection test
- CONTRIBUTING.md invariant update

**Credential boundary:** GUI → RPC → service → keyring. GUI never touches key plaintext (key travels over local Unix socket to service, which stores it in keyring).

**Not in Phase 1:** profile management, sidecar spawning, model inference.

---

### Phase 2: Subagent Monitoring Page

**Gap closed:** GUI subagent surface (Gap 3, read-only portion)

**Deliverables:**
- New RPC: `subagent.runtime_status` — single-snapshot aggregate read model
- GUI: new "Subagents" sidebar entry (TOOLS group)
- Page layout: pressure summary + active subagents (logical entities) + task history (recent 20, `created_at_ms desc`) + sidecar workers (process entities)

**`subagent.runtime_status` shape:**
```
{
  pressure_gate: {
    level: "normal" | "throttled" | "evicting" | "restarting",
    memory_used_pct: u32,
    hot_sessions_total: u32,
    hot_sessions_limit: u32,
  },
  subagents: [
    { name, status, task_count, last_task_at_ms, last_task_status }
  ],
  tasks_recent: [
    { task_id, subagent_name, status, created_at_ms, error }
  ],
  workers: [
    { provider_id, state: "running"|"stopped"|"stale", pid, uptime_seconds, hot_sessions }
  ]
}
```

**Constraints:**
- Single-snapshot semantics: all fields in one response come from the same moment.
- `tasks_recent`: fixed limit 20, ordered by `created_at_ms desc`.
- Strictly read-only: no hibernate/delete/retry buttons. Existing `subagent.hibernate` / `subagent.delete` RPCs are NOT called from GUI.
- Subagent rows show logical entities only (no pid). Worker rows show process entities only (no subagent name).
- `settings.diagnostics` is for doctor-level health checks (low frequency); `subagent.runtime_status` is for runtime monitoring (5s poll). No overlap.
- **Before Phase 3 lands**, `workers[]` may be empty (no real sidecar workers running). Phase 2 accepts this gracefully — workers section shows "No active sidecar workers." The page is still useful for subagent/task monitoring.
- **Pressure aggregation is display-only**: `pressure_gate.memory_used_pct` is the system-wide memory usage (not per-provider). `hot_sessions_total` is a sum across all active workers. Phase 3 applies resource limits per-worker; Phase 2's aggregate summary is informational and does not imply a global pressure controller.

**Activity page:** subagent token consumption merges into existing Activity page after Phase 3 produces real usage data. Phase 2 task history shows task-level metadata only (no token counts).

---

### Phase 3: Pi SDK Real Integration

**Gap closed:** Pi SDK integration (Gap 1) + end-to-end delegate flow (Gap 5)

**Deliverables:**
- `@earendil-works/pi-coding-agent` as runtime dependency of `apps/pi-sidecar`
- Real `PiSession` wrapper class (Node side) replacing the data interface
- Real `turn_auto` handler replacing mock
- Multi-provider worker management: **one `PiSidecarSupervisor` instance per active provider** (reuses existing single-worker code, avoids rewriting resource/pressure aggregation). A `WorkerPool` owns `HashMap<ProviderId, Arc<PiSidecarSupervisor>>`. Resource limits (`memory_hard_limit_mb`) apply **per-worker** (per-provider), not aggregated. `SidecarTaskExecutor` selects supervisor by `profile.provider_id`.
- Usage bridge: subagent usage is written to the **unified `usage_events` table** (not only `subagent_usage_records`). The Rust executor normalizes the sidecar's raw usage into `NormalizedUsageEvent` with `client_kind: "subagent"` and writes via the store writer. Existing `subagent_usage_records` writes remain for internal bookkeeping.
- Usage normalization + price catalog lookup in Rust executor
- esbuild switched to CJS format (fixed, not optional)
- **Risk flag**: verify `@earendil-works/pi-coding-agent` and all transitive deps are CJS-importable before committing to single-file bundle. If SDK is ESM-only, evaluate dynamic `import()` inside CJS or reconsider format.

**Worker model (Rust):**
```
Worker {
  provider_id: String,
  process: Option<Child>,
  rpc_client: Option<SidecarRpcClient>,
  stale: bool,
  spawned_at: Instant,
}
// No session_pool in Rust — sessions live in Node sidecar only
```

**Worker lifecycle:**
- delegate → resolve profile.provider_id → ensure worker running (stale → kill + respawn with fresh keyring read)
- auth failure (401) → immediately kill worker + remove from HashMap (hard invalidation)
- provider/key change → worker marked stale → rebuilt on next delegate
- idle TTL → worker killed + removed

**Error handling (Phase 3 — no auto-retry):**

| Scenario | Task status | Worker action | Auto-retry |
|---|---|---|---|
| auth failure (401) | `failed` | immediately kill + remove | ❌ |
| rate limit (429) | `failed` | keep | ❌ (deferred) |
| timeout | `failed` | keep | ❌ (deferred) |
| sidecar crash | `failed` | kill + remove | ❌ (deferred) |
| network unreachable | `failed` | keep | ❌ (deferred) |

All errors classified (auth/rate_limit/timeout/crash/network) and recorded in task error field for future retry phase.

**Existing crash-recovery:** the current `PiSidecarSupervisor` has a 5-min rolling restart window with a 3-restart cap. For Phase 3 MVP, auth-fail bypasses this (hard kill). Non-auth crashes (e.g., node SIGSEGV) still go through the existing restart-window logic within that provider's supervisor — but task is marked `failed` (no auto-retry of the task itself). The restart window prevents infinite crash loops at the process level; the task-level retry is deferred.

**Sidecar spawn env injection (explicit):** the env var name (e.g., `DEEPSEEK_API_KEY`) is non-secret config stored in `settings.toml` as `api_key_env_name`. The *value* is the keychain secret. At spawn time, service reads both: `env.insert(provider.api_key_env_name, keyring.get(id))` and `env.insert(provider.base_url_env_name.unwrap_or("OPENAI_BASE_URL"), provider.base_url)`. The sidecar reads `process.env[api_key_env_name]` and `process.env[base_url_env_name]`.

**Usage flow:**
```
Node sidecar returns raw provider usage: { input_tokens, output_tokens }
  → may be partially missing; Rust handles conservatively (never fabricates precision)
  → Rust normalizes to NormalizedUsageEvent {
      client_kind: "subagent",
      model: actual returned model (profile.model as fallback),
      total_tokens: input + output (computed in Rust),
      input_tokens, output_tokens,
      cost_usd: price_catalog.lookup(model, input, output),
      source_path: cwd,
      session_id: subagent_id,
    }
  → writes to usage_events via store writer
  → Activity page and Overview page pick it up naturally
```

**Node-side responsibilities:**
- Session pool (LRU<SubagentId, PiSession>)
- Pi SDK calls (`createAgentSession`, `sendTurn`)
- Returns raw usage only (no price calculation, no normalization)

**`prepare_hibernate`:** MVP uses context summary (not real SDK memory compaction). Deferred to future phase.

**Protocol:** only `openai_compatible` in Phase 3 MVP. Non-OpenAI providers deferred.

**Streaming:** not in MVP. Non-streaming only.

**esbuild:** CJS format, fixed. `cross-spawn` bundles into single file. No runtime `node_modules` dependency.

---

### Phase 4: Profile/Model Configuration UI

**Gap closed:** Profile/model config UI (Gap 6)

**Deliverables:**
- Wire DTO extension: `SettingsSnapshotDto` gains `subagent: { enabled, profiles[] }`
- Dedicated RPC: `profile.create / update / delete` (not `settings.update` — profiles are structured sub-resources)
- Service canonicalization: on config load, fill missing built-in profiles (never overwrite user edits)
- GUI: profile management section in the Providers page

**RPC design:**
- Read: `settings.snapshot` returns `subagent.profiles[]`
- Write: `profile.create` / `profile.update` (partial/patch semantics — can update single field without sending full object) / `profile.delete`
- `profile.delete` rejects built-in profiles

**Built-in profile canonicalization:**
- Service ensures 3 built-in profiles exist on every config load.
- Missing → fill with defaults. Present → leave untouched (even if user changed provider/model).
- Frontend always receives a complete profile list.

**Disabled provider constraint:**
- Existing bindings to disabled providers: persist + show ⚠ warning.
- New bindings to disabled providers: blocked (dropdown excludes disabled providers; service rejects save).
- Saving a profile pointing to disabled provider: validation error.

**Stale model handling:**
- If `profile.model ∉ provider.models` (after provider model list changed): UI shows invalid/stale state, requires re-selection before save.

**MVP UI scope:**
- Editable: `provider_id` (dropdown, only enabled), `model` (cascade-filtered by selected provider).
- Read-only display: `tools`, `context_budget_tokens`, `timeout_seconds` (collapsed in advanced section).
- Immutable: `id`, `is_builtin`.

---

### Phase 5: Sidecar Bundling for Distribution

**Gap closed:** Sidecar bundling (Gap 4)

**Deliverables:**
- Sidecar build, bundling, and signing logic added to `packaging/macos/scripts/package_dmg.sh` (the existing single source of truth for macOS packaging, shared between local rehearsal and CI release). `release.yml` calls the script — no parallel packaging logic in the workflow.
- Node.js runtime binaries bundled into `.app`
- manifest.json generated at build time
- Code signing (node binary signed with entitlements)
- Runtime path resolution (persisted locator, no heuristics)

**.app structure:**
```
Busytok.app/Contents/Resources/pi-sidecar/
  pi-sidecar.bundle.js
  manifest.json
  node/aarch64/node
  node/x86_64/node
```

**manifest.json:**
```json
{
  "version": "1",
  "protocol_version": 1,
  "bundle": "pi-sidecar.bundle.js",
  "node_runtime_version": "22.6.0"
}
```

**Runtime path resolution (no heuristics):**
- **Packaged build**: on first launch and on every GUI startup, the GUI process resolves the absolute sidecar resource path (`<.app>/Contents/Resources/pi-sidecar/` via Tauri resource API) and **persists it as a runtime locator** that the service can read independently (e.g., a field in `settings.toml` or a marker file). The GUI injection is a **refresh mechanism**, not the sole source — the service reads the persisted locator on its own startup (including login auto-start and CLI-only invocation without GUI). This avoids `current_exe()` path guessing and ensures the daemon can resolve the path even when the GUI is not running.
- **Dev mode**: `runtime_dir` from `settings.toml` explicit override, or fallback to `apps/pi-sidecar/dist/` (existing behavior).
- No `current_exe()` path guessing. No dual-channel competition — packaged mode always uses the persisted locator; dev mode always uses settings/fallback.

**Signing order (must be sequential):**
1. Build `.app` via Tauri
2. Copy sidecar resources (bundle, manifest, node binaries) into `.app/Contents/Resources/pi-sidecar/`
3. Sign nested node binaries with Developer ID + entitlements (`allow-jit`, `allow-unsigned-executable-memory`)
4. Sign outer `.app` bundle (re-seal)
5. Notarize + staple
6. Package DMG

**Entitlements:** applied to the `node` binary's code signature, NOT to `busytok-service`'s entitlement plist. Start with **`allow-jit` only** (minimal privilege). Only add `allow-unsigned-executable-memory` if notarized runtime testing proves Node 22 requires it under hardened runtime. Verify empirically before broadening.

**Dual-architecture principle:**
- Both `aarch64` and `x86_64` node binaries bundled for distribution compatibility.
- Runtime selects ONLY the current host architecture.
- Never attempts Rosetta cross-architecture fallback.

**DMG size impact:** ~95MB increase (15MB bundle + 40MB × 2 node binaries). Current ~30MB → ~125MB.

**Doctor checks (all pass after Phase 5):**

The current `run_subagent_doctor` emits 10 checks. After Phase 5, all 10 should pass (currently 3 fail due to missing Pi bundle). No new check is added — the "11th" in earlier drafts was an error. Final set:

| # | Check | Current | After Phase 5 |
|---|---|---|---|
| 1 | `service_running` | ✅ | ✅ |
| 2 | `sqlite_readable` | ✅ | ✅ |
| 3 | `sidecar_launchable` | ✅ | ✅ |
| 4 | `bundled_node_arch` | ✗ | ✅ node binary at correct arch |
| 5 | `bundle_manifest_readable` | ✗ | ✅ manifest exists + valid JSON |
| 6 | `protocol_version` | ⚠ disabled | ✅ enabled + handshake |
| 7 | `default_model_config` | ✅ | ✅ |
| 8 | `pi_runtime_installed` | ✗ | ✅ node + bundle both present |
| 9 | `artifact_store_writable` | ✅ | ✅ |
| 10 | `resource_policy_valid` | ✅ | ✅ |

Acceptance: "all 10 doctor checks pass" (not 11).

**Verify checks (new in `release_script_smoke.sh`):**
- pi-sidecar.bundle.js exists
- manifest.json exists + valid JSON
- node binary exists + executable for current arch

**Deferred:**
- Node SEA (Single Executable Application)
- Per-architecture DMG distribution
- Node version auto-update
- CI-level end-to-end delegate test on packaged build

---

## 5. Phase Dependency Graph

```
Phase 1 (Credential Foundation)
  ↓ gates
Phase 3 (Pi SDK Integration) ←── needs keyring to inject keys
Phase 4 (Profile Config UI)  ←── needs provider model to bind profiles

Phase 2 (Subagent Monitoring) ←── independent, can start immediately
Phase 5 (Sidecar Bundling)    ←── needs Phase 1+3 production-ready

Recommended execution order: 1 → 2 → 3 → 4 → 5
```

---

## 6. Acceptance Criteria

### Phase 1
- [ ] User can create/edit/delete a provider in the GUI
- [ ] API key is stored in macOS Keychain (verified: `security find-generic-password -s com.busytok.providers`)
- [ ] API key never persists to disk (not in settings.toml, not in logs). Key only travels via local Unix socket IPC from GUI to service during provider setup, and from service to sidecar via env var at spawn.
- [ ] Provider connection test works (test_connection RPC)
- [ ] CONTRIBUTING.md updated

### Phase 2
- [ ] Subagents page renders in sidebar
- [ ] `subagent.runtime_status` returns single-snapshot data
- [ ] Active subagents, task history (20 items), pressure status, worker health all render
- [ ] No write-action buttons on the page
- [ ] 5s auto-refresh works

### Phase 3
- [ ] `turn_auto` calls real model API (verified: delegate returns real model output)
- [ ] One worker process per provider (verified: multiple providers → multiple node processes)
- [ ] Auth failure immediately kills worker
- [ ] Usage recorded in unified `usage_events` with `client_kind: "subagent"` (verified: `SELECT count(*) FROM usage_events WHERE client_kind='subagent'`)
- [ ] Activity page shows subagent token consumption
- [ ] esbuild produces CJS bundle

### Phase 4
- [ ] Built-in profiles visible in Providers page
- [ ] User can bind profile to provider + model
- [ ] Disabled provider excluded from new bindings
- [ ] Stale model shows invalid state
- [ ] Service canonicalizes missing built-in profiles on load
- [ ] `profile.update` supports partial/patch semantics

### Phase 5
- [ ] DMG includes pi-sidecar.bundle.js + manifest.json + node binaries
- [ ] Fresh install: `busytok doctor` passes all 10 checks
- [ ] Fresh install: `busytok delegate` spawns sidecar and returns model output
- [ ] Node binary signed with JIT entitlements
- [ ] Runtime selects correct arch node (no Rosetta fallback)
- [ ] DMG size < 150MB
