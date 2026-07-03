# Phase 4: Profile/Model Configuration UI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a profile management section to the Providers page that lets users bind profiles to providers+models, with service-side canonicalization of built-in profiles, validation (disabled-provider rejection, stale-model detection), and dedicated `profile.create/update/delete` RPCs.

**Architecture:** Profiles remain a `HashMap<String, SubagentProfileConfig>` under `settings.subagent.profiles` in TOML. The service canonicalizes built-in profiles on load (fill missing, never overwrite). Reads flow through `settings.snapshot` (extended with `subagent.profiles[]`). Writes flow through dedicated `profile.*` RPCs (patch semantics for update). The existing `profile_provider_id()` helper at supervisor.rs:5719 is the SSOT for provider extraction and is reused, not reimplemented.

**Tech Stack:** Rust (busytok-config, busytok-protocol, busytok-control, busytok-runtime), React/TypeScript (apps/gui), ts-rs for DTO generation.

## Global Constraints

- Spec source: `docs/superpowers/specs/2026-06-29-subagent-full-integration-design.md` §4 Phase 4 (lines 305-336), §3.4 Constraints (lines 139-149), §3.5 Config Migration (lines 150-157), §6 Phase 4 Acceptance (lines 464-470).
- Built-in profiles: `pi/search-cheap`, `pi/review-cheap`, `pi/plan-cheap` — defined in `default_profiles()` at `crates/busytok-config/src/lib.rs:400`.
- `provider_id: Option<String>` already exists on `SubagentProfileConfig` (Phase 3). `profile_provider_id()` helper already exists at `supervisor.rs:5719`.
- Read path: `settings.snapshot` returns `subagent.profiles[]` (NOT a separate `profile.list` RPC).
- Write path: `profile.create` / `profile.update` (patch semantics) / `profile.delete` (rejects built-in).
- Built-in profiles: `is_builtin` is derived (not stored) — true if name ∈ `{pi/search-cheap, pi/review-cheap, pi/plan-cheap}`.
- MVP UI scope: editable = `provider_id` + `model`; read-only display = `tools`, `context_budget_tokens`, `timeout_seconds`; immutable = `id`, `is_builtin`.
- Disabled provider constraint: new bindings to disabled providers are blocked; existing bindings persist with ⚠ warning.
- Stale model: if `profile.model ∉ provider.models`, UI shows invalid state, requires re-selection before save.
- Logging: use `tracing::info!`/`tracing::warn!` with `event_code` pattern (e.g., `profile.created`, `profile.update.rejected`).
- Tests: TDD — write failing test first, then implement. Target >90% coverage on new code.
- DRY: reuse `provider_to_dto` pattern, `provider_changed()` side-effect pattern (kill + remove worker from pool on provider mutation), `SettingsValidationErrorDto` validation-error pattern. Note: profile mutations do NOT call `provider_changed()` — only provider mutations invalidate workers.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/busytok-config/src/lib.rs` | Config schema + load/save + built-in canonicalization | Modify |
| `crates/busytok-protocol/src/dto.rs` | Wire DTOs for profiles + SettingsSnapshotDto extension | Modify |
| `crates/busytok-protocol/src/methods.rs` | Method manifest | Modify |
| `crates/busytok-protocol/src/ts.rs` | Register new DTOs for TypeScript export | Modify |
| `crates/busytok-control/src/dispatch.rs` | RuntimeControl trait + dispatch + stubs + forwarders | Modify |
| `crates/busytok-runtime/src/supervisor.rs` | Profile CRUD handlers + settings_snapshot extension | Modify |
| `crates/busytok-runtime/tests/supervisor_control.rs` | Profile CRUD + validation tests | Modify |
| `crates/busytok-config/tests/subagent_settings.rs` | Canonicalization tests | Modify |
| `apps/gui/src/api/busytokClient.ts` | Profile client methods (incl. `profileCreate` — exposed for future use, not wired to MVP UI) | Modify |
| `apps/gui/src/api/useBusytokData.ts` | Profile hooks (`useProfileMutations` returns `createProfile`/`updateProfile`/`deleteProfile`; `createProfile` is exported for future use but MVP `ProfilesSection` only uses update/delete — built-in profiles ship unbound and spec §4 lists no "create profile" UI) | Modify |
| `apps/gui/src/components/ProfilesSection.tsx` | Profile management UI section | Create |
| `apps/gui/src/components/ProfilesSection.test.tsx` | Profile UI tests | Create |
| `apps/gui/src/pages/ProvidersPage.tsx` | Import + render ProfilesSection | Modify |
| `apps/gui/src/pages/ProvidersPage.test.tsx` | Update mocks for ProfilesSection | Modify |
| `apps/gui/src/App.test.tsx` | Patch `SettingsSnapshotDto` fixture with `subagent` field | Modify |
| `apps/gui/src/pages/SettingsPageCoverage.test.tsx` | Patch `snapshot()` fixture with `subagent` field | Modify |

---

### Task 1: Config — Built-in Profile Canonicalization + `is_builtin_profile` Helper

**Files:**
- Modify: `crates/busytok-config/src/lib.rs`
- Modify: `crates/busytok-config/tests/subagent_settings.rs`

**Interfaces:**
- Produces: `pub fn is_builtin_profile(name: &str) -> bool` — true if name ∈ built-in set.
- Produces: `pub fn canonicalize_builtin_profiles(&mut self)` method on `BusytokSettings` — fills missing built-in profiles, never overwrites present ones.
- Consumes: existing `default_profiles()` function.

- [ ] **Step 1: Write failing test for `is_builtin_profile`**

Add to `crates/busytok-config/tests/subagent_settings.rs`:

```rust
#[test]
fn is_builtin_profile_recognizes_builtins() {
    assert!(busytok_config::is_builtin_profile("pi/search-cheap"));
    assert!(busytok_config::is_builtin_profile("pi/review-cheap"));
    assert!(busytok_config::is_builtin_profile("pi/plan-cheap"));
    assert!(!busytok_config::is_builtin_profile("pi/patch-small"));
    assert!(!busytok_config::is_builtin_profile("my-custom-profile"));
    assert!(!busytok_config::is_builtin_profile(""));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p busytok-config --test subagent_settings is_builtin_profile_recognizes_builtins`
Expected: FAIL — `is_builtin_profile` not found.

- [ ] **Step 3: Implement `is_builtin_profile`**

In `crates/busytok-config/src/lib.rs`, after `default_profiles()` (line ~444), add:

```rust
/// Returns true if `name` is one of the 3 built-in profiles.
///
/// Used by the runtime (to reject `profile.delete` on built-in profiles)
/// and by the DTO mapper (to set `is_builtin: bool` on `ProfileDto`).
/// Single source of truth — do NOT duplicate this check elsewhere.
pub fn is_builtin_profile(name: &str) -> bool {
    matches!(
        name,
        "pi/search-cheap" | "pi/review-cheap" | "pi/plan-cheap"
    )
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p busytok-config --test subagent_settings is_builtin_profile_recognizes_builtins`
Expected: PASS.

- [ ] **Step 5: Write failing test for canonicalization**

Add to `crates/busytok-config/tests/subagent_settings.rs`:

```rust
#[test]
fn canonicalize_fills_missing_builtins_without_overwriting_user_edits() {
    let toml_str = r#"
[subagent.profiles."pi/search-cheap"]
model = "user-customized-model"
provider_id = "my-provider"
tools = ["read"]
context_budget_tokens = 9999
timeout_seconds = 42
write_access = false
"#;
    let mut settings =
        busytok_config::BusytokSettings::load_from_str(toml_str).unwrap();
    // Only pi/search-cheap is present; pi/review-cheap and pi/plan-cheap are missing.
    assert_eq!(settings.subagent.profiles.len(), 1);

    settings.canonicalize_builtin_profiles();

    // All 3 built-in profiles now present.
    assert_eq!(settings.subagent.profiles.len(), 3);
    assert!(settings.subagent.profiles.contains_key("pi/search-cheap"));
    assert!(settings.subagent.profiles.contains_key("pi/review-cheap"));
    assert!(settings.subagent.profiles.contains_key("pi/plan-cheap"));

    // pi/search-cheap was NOT overwritten — user edits preserved.
    let search = &settings.subagent.profiles["pi/search-cheap"];
    assert_eq!(search.model, "user-customized-model");
    assert_eq!(search.provider_id.as_deref(), Some("my-provider"));
    assert_eq!(search.context_budget_tokens, 9999);

    // pi/review-cheap was filled with defaults.
    let review = &settings.subagent.profiles["pi/review-cheap"];
    assert_eq!(review.model, "qwen-coder");
    assert_eq!(review.provider_id, None);
    assert_eq!(review.context_budget_tokens, 5000);
}
```

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p busytok-config --test subagent_settings canonicalize_fills_missing_builtins_without_overwriting_user_edits`
Expected: FAIL — `canonicalize_builtin_profiles` not found.

- [ ] **Step 7: Implement `canonicalize_builtin_profiles`**

In `crates/busytok-config/src/lib.rs`, inside `impl BusytokSettings` (after the `save` method, ~line 510), add:

```rust
/// Ensure all 3 built-in profiles exist. Missing ones are filled with
/// defaults; present ones are left untouched (even if user modified them).
///
/// Called by `load()` after timezone canonicalization. Spec §4 Phase 4:
/// "Service ensures 3 built-in profiles exist on every config load.
/// Missing → fill with defaults. Present → leave untouched."
pub fn canonicalize_builtin_profiles(&mut self) {
    let builtins = default_profiles();
    let mut filled = Vec::new();
    for (name, cfg) in &builtins {
        if !self.subagent.profiles.contains_key(name) {
            self.subagent.profiles.insert(name.clone(), cfg.clone());
            filled.push(name.clone());
        }
    }
    if !filled.is_empty() {
        tracing::info!(
            event_code = "profile.builtin_canonicalized",
            filled = ?filled,
            "filled missing built-in profiles during config load"
        );
    }
}
```

- [ ] **Step 8: Run test to verify it passes**

Run: `cargo test -p busytok-config --test subagent_settings canonicalize_fills_missing_builtins_without_overwriting_user_edits`
Expected: PASS.

- [ ] **Step 9: Hook canonicalization into `load()`**

In `BusytokSettings::load()` (line ~462), after the timezone canonicalization block (after line ~488, before `Ok(settings)`), add:

```rust
                // Canonicalize built-in profiles: fill missing, never overwrite.
                settings.canonicalize_builtin_profiles();
```

- [ ] **Step 10: Write test for load-time canonicalization**

Add to `crates/busytok-config/tests/subagent_settings.rs`:

```rust
#[test]
fn load_canonicalizes_missing_builtin_profiles() {
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = busytok_config::BusytokPaths::for_test(tmp.path());
    let config_dir = paths.config_dir();
    std::fs::create_dir_all(config_dir).unwrap();
    // Write a config with only pi/search-cheap (missing the other 2 builtins).
    std::fs::write(
        config_dir.join("settings.toml"),
        r#"
[subagent.profiles."pi/search-cheap"]
model = "deepseek-chat"
"#,
    )
    .unwrap();

    let settings = busytok_config::BusytokSettings::load(&paths).unwrap();
    assert_eq!(settings.subagent.profiles.len(), 3);
    assert!(settings.subagent.profiles.contains_key("pi/review-cheap"));
    assert!(settings.subagent.profiles.contains_key("pi/plan-cheap"));
}
```

- [ ] **Step 11: Write idempotency test**

Add to `crates/busytok-config/tests/subagent_settings.rs`:

```rust
#[test]
fn canonicalize_is_idempotent() {
    let mut settings = busytok_config::BusytokSettings::default();
    settings.canonicalize_builtin_profiles();
    let count_after_first = settings.subagent.profiles.len();
    // Calling again must not duplicate or remove profiles.
    settings.canonicalize_builtin_profiles();
    assert_eq!(settings.subagent.profiles.len(), count_after_first);
    // Each built-in appears exactly once.
    for name in &["pi/search-cheap", "pi/review-cheap", "pi/plan-cheap"] {
        assert_eq!(settings.subagent.profiles.contains_key(*name), true);
    }
}
```

- [ ] **Step 12: Run all config tests**

Run: `cargo test -p busytok-config`
Expected: All tests PASS.

- [ ] **Step 13: Commit**

```bash
git add crates/busytok-config/src/lib.rs crates/busytok-config/tests/subagent_settings.rs
git commit -m "feat(config): add built-in profile canonicalization + is_builtin_profile helper

Phase 4 Task 1: Service canonicalizes missing built-in profiles on
config load (fill missing, never overwrite user edits). Adds
is_builtin_profile() as the single source of truth for built-in
profile detection, used by profile.delete rejection and DTO mapping."
```

---

### Task 2: Protocol — Profile DTOs + SettingsSnapshotDto Extension + Method Manifest

**Files:**
- Modify: `crates/busytok-protocol/src/dto.rs`
- Modify: `crates/busytok-protocol/src/methods.rs`
- Modify: `crates/busytok-protocol/src/ts.rs`

**Interfaces:**
- Produces: `ProfileDto`, `ProfileCreateRequestDto`, `ProfileUpdateRequestDto`, `ProfileDeleteRequestDto`, `SettingsSubagentDto`.
- Produces: `SettingsSnapshotDto.subagent: SettingsSubagentDto` (new field).
- Produces: `profile.create` / `profile.update` / `profile.delete` in method manifest.
- Consumes: `is_builtin_profile` from Task 1 (used by the supervisor to populate `ProfileDto.is_builtin`).

- [ ] **Step 1: Add Profile DTOs to `dto.rs`**

In `crates/busytok-protocol/src/dto.rs`, after the provider DTOs block (after line ~1607), add:

```rust
// ── Profiles (Phase 4: Profile/Model Configuration UI) ──────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProfileDto {
    pub id: String,
    /// True if this is one of the 3 built-in profiles (pi/search-cheap, etc.).
    /// Derived by the service from `is_builtin_profile()` — not stored in config.
    pub is_builtin: bool,
    /// Provider this profile runs on. None = unbound (delegate will reject).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub model: String,
    pub tools: Vec<String>,
    pub context_budget_tokens: u32,
    pub timeout_seconds: u64,
    pub write_access: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProfileCreateRequestDto {
    pub id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_budget_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_access: Option<bool>,
}

/// Patch semantics: None = leave unchanged. For provider_id, Some("") = unbind.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProfileUpdateRequestDto {
    pub id: String,
    /// Some("openai") = bind to openai; Some("") = unbind; None = unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_budget_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_access: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProfileDeleteRequestDto {
    pub id: String,
}
```

- [ ] **Step 2: Add `SettingsSubagentDto` and extend `SettingsSnapshotDto`**

In `dto.rs`, before `SettingsSnapshotDto` (line ~827), add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct SettingsSubagentDto {
    pub enabled: bool,
    pub profiles: Vec<ProfileDto>,
}
```

Then modify `SettingsSnapshotDto` to add the `subagent` field:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SettingsSnapshotDto {
    pub timezone: String,
    pub week_starts_on: WeekdayIndexDto,
    pub discovery: SettingsDiscoveryDto,
    pub privacy: SettingsPrivacyDto,
    pub diagnostics: SettingsDiagnosticsDto,
    pub recovery_actions: Vec<SettingsRecoveryActionDto>,
    pub prompt_palette_default_action: PromptActionDto,
    pub subagent: SettingsSubagentDto,
}
```

- [ ] **Step 3: Add method manifest entries**

In `crates/busytok-protocol/src/methods.rs`, after the provider entries (after line ~56), add:

```rust
        // Profiles (Phase 4: Profile/Model Configuration UI)
        "profile.create".to_string(),
        "profile.update".to_string(),
        "profile.delete".to_string(),
```

- [ ] **Step 4: Add method manifest test**

In `methods.rs`, add to the test module:

```rust
#[test]
fn method_manifest_contains_profile_methods() {
    let manifest = method_manifest();
    assert!(manifest.contains(&"profile.create".to_string()));
    assert!(manifest.contains(&"profile.update".to_string()));
    assert!(manifest.contains(&"profile.delete".to_string()));
}
```

- [ ] **Step 5: Register new DTOs for TypeScript export in `ts.rs`**

The TS type generator (`crates/busytok-protocol/src/ts.rs`) maintains an explicit `type_defs` vec listing every DTO to export via `decl()`. The new DTOs must be registered or `@busytok/protocol-types` won't contain them and the regenerated `SettingsSnapshotDto` TS type will reference an undefined `SettingsSubagentDto`.

In `crates/busytok-protocol/src/ts.rs`, in the `type_defs` vec:

1. Add `SettingsSubagentDto` **before** `SettingsSnapshotDto` (currently at line ~115), since `SettingsSnapshotDto` now references it:
```rust
            dto::SettingsSubagentDto::decl(),
            dto::SettingsSnapshotDto::decl(),
```

2. Add the 4 Profile DTOs near the provider DTOs (after line ~198, after `ProviderTestConnectionResponseDto::decl()`):
```rust
            // Profiles (Phase 4: Profile/Model Configuration UI)
            dto::ProfileDto::decl(),
            dto::ProfileCreateRequestDto::decl(),
            dto::ProfileUpdateRequestDto::decl(),
            dto::ProfileDeleteRequestDto::decl(),
```

- [ ] **Step 6: Build to verify DTOs compile**

Run: `cargo check -p busytok-protocol 2>&1 | head -5`
Expected: `busytok-protocol` parses cleanly (the DTO definitions themselves are syntactically correct).

The new required `subagent` field on `SettingsSnapshotDto` breaks **all** existing construction sites. These are repaired in later tasks — do not expect the wider workspace to compile yet:
- `crates/busytok-control/src/dispatch.rs:934` and `:966` — two `TestRuntimeControl` stub literals → patched in Task 3 Step 5.
- `crates/busytok-runtime/src/supervisor.rs:4164` — runtime `settings_snapshot` handler → patched in Task 4 Step 4.
- `apps/gui/src/App.test.tsx:256` and `apps/gui/src/pages/SettingsPageCoverage.test.tsx:112` — GUI test fixtures → patched in Task 5 Step 3 (after ts-rs regenerates types).

- [ ] **Step 7: Commit**

```bash
git add crates/busytok-protocol/src/dto.rs crates/busytok-protocol/src/methods.rs crates/busytok-protocol/src/ts.rs
git commit -m "feat(protocol): add Profile DTOs + extend SettingsSnapshotDto with subagent section

Phase 4 Task 2: Wire DTOs for profile CRUD (ProfileDto,
ProfileCreateRequestDto, ProfileUpdateRequestDto with patch semantics,
ProfileDeleteRequestDto). SettingsSnapshotDto gains subagent: { enabled,
profiles[] } per spec §4 Phase 4. Method manifest gains profile.create/
update/delete."
```

---

### Task 3: Control — RuntimeControl Trait + Dispatch + Stubs + Forwarders

**Files:**
- Modify: `crates/busytok-control/src/dispatch.rs`

**Interfaces:**
- Produces: `profile_create`, `profile_update`, `profile_delete` trait methods on `RuntimeControl`.
- Produces: `profile.create` / `profile.update` / `profile.delete` dispatch arms.
- Produces: `TestRuntimeControl` stub implementations.
- Produces: `Arc<dyn RuntimeControl>` forwarder implementations.

- [ ] **Step 1: Add trait method declarations**

In `crates/busytok-control/src/dispatch.rs`, after the provider trait methods (after line ~211), add:

```rust
    // Profiles (Phase 4: Profile/Model Configuration UI)
    async fn profile_create(&self, req: ProfileCreateRequestDto) -> Result<ProfileDto>;
    async fn profile_update(&self, req: ProfileUpdateRequestDto) -> Result<ProfileDto>;
    async fn profile_delete(&self, req: ProfileDeleteRequestDto) -> Result<()>;
```

Also add the imports at the top of the file (after the existing provider DTO imports):

```rust
use busytok_protocol::dto::{
    ProfileCreateRequestDto, ProfileDeleteRequestDto, ProfileDto, ProfileUpdateRequestDto,
    SettingsSubagentDto,
};
```

- [ ] **Step 2: Add dispatch arms**

In the `match request.method.as_str()` block, after the `provider.test_connection` arm (before the `_ =>` fallback), add:

```rust
            // Profiles (Phase 4: Profile/Model Configuration UI)
            "profile.create" => {
                let req: ProfileCreateRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for profile.create: {e}"))?;
                let dto = self.runtime.profile_create(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "profile.update" => {
                let req: ProfileUpdateRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for profile.update: {e}"))?;
                let dto = self.runtime.profile_update(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "profile.delete" => {
                let req: ProfileDeleteRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for profile.delete: {e}"))?;
                self.runtime.profile_delete(req).await?;
                ControlResponse::ok(serde_json::to_value(())?)
            }
```

- [ ] **Step 3: Add `TestRuntimeControl` stub implementations**

After the provider stubs (after line ~1201), add:

```rust
    // ── Profiles (Phase 4: Profile/Model Configuration UI) ──────────

    async fn profile_create(&self, _req: ProfileCreateRequestDto) -> Result<ProfileDto> {
        anyhow::bail!("not yet implemented")
    }
    async fn profile_update(&self, _req: ProfileUpdateRequestDto) -> Result<ProfileDto> {
        anyhow::bail!("not yet implemented")
    }
    async fn profile_delete(&self, _req: ProfileDeleteRequestDto) -> Result<()> {
        anyhow::bail!("not yet implemented")
    }
```

- [ ] **Step 4: Add `Arc<dyn RuntimeControl>` forwarders**

After the provider forwarders (after line ~1412), add:

```rust
    async fn profile_create(&self, req: ProfileCreateRequestDto) -> Result<ProfileDto> {
        (**self).profile_create(req).await
    }
    async fn profile_update(&self, req: ProfileUpdateRequestDto) -> Result<ProfileDto> {
        (**self).profile_update(req).await
    }
    async fn profile_delete(&self, req: ProfileDeleteRequestDto) -> Result<()> {
        (**self).profile_delete(req).await
    }
```

- [ ] **Step 5: Patch existing `SettingsSnapshotDto` literals in `TestRuntimeControl`**

Task 2 added a required `subagent: SettingsSubagentDto` field to `SettingsSnapshotDto`. Two existing stubs in `dispatch.rs` construct this DTO literally and must be patched before the control crate will compile.

In `crates/busytok-control/src/dispatch.rs`, find the `settings_snapshot` stub at **line ~934** and add the `subagent` field before the closing `}`:

```rust
            recovery_actions: vec![],
            subagent: SettingsSubagentDto {
                enabled: true,
                profiles: vec![],
            },
        }))
    }
```

Then find the `settings_update` stub at **line ~966** and add the same field:

```rust
            recovery_actions: vec![],
            subagent: SettingsSubagentDto {
                enabled: true,
                profiles: vec![],
            },
        }))
    }
```

(`SettingsSubagentDto` was already imported in Step 1.)

- [ ] **Step 6: Build control crate**

Run: `cargo check -p busytok-control 2>&1 | tail -5`
Expected: Control crate compiles cleanly. (Runtime crate will still fail — Task 4 implements the `profile_*` trait methods.)

- [ ] **Step 7: Commit**

```bash
git add crates/busytok-control/src/dispatch.rs
git commit -m "feat(control): add profile_create/update/delete to RuntimeControl trait

Phase 4 Task 3: Wire profile CRUD through the control dispatch layer
(trait declaration + dispatch arms + TestRuntimeControl stubs + Arc
forwarder). Patches two existing SettingsSnapshotDto stub literals with
the new subagent field. Read path stays via settings.snapshot; write
path uses dedicated profile.* RPCs."
```

---

### Task 4: Runtime — Profile Handlers + `settings_snapshot` Extension (TDD)

**Files:**
- Modify: `crates/busytok-runtime/src/supervisor.rs`
- Modify: `crates/busytok-runtime/tests/supervisor_control.rs`

**Interfaces:**
- Produces: `profile_to_dto()` free function (mirrors `provider_to_dto`).
- Produces: `profile_create`, `profile_update`, `profile_delete` handler implementations.
- Produces: Extended `settings_snapshot()` that includes `subagent: { enabled, profiles[] }`.
- Consumes: `is_builtin_profile()` from Task 1.
- Consumes: `profile_provider_id()` existing helper at supervisor.rs:5719.
- Consumes: `provider_to_dto()` pattern for the `profile_to_dto` mirror.

- [ ] **Step 1: Write failing test for `profile_crud_round_trips`**

Add to `crates/busytok-runtime/tests/supervisor_control.rs` (after the provider CRUD tests):

```rust
#[tokio::test]
async fn profile_crud_round_trips() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Built-in profiles exist from default_settings.
    let snapshot = sup.settings_snapshot().await.unwrap();
    assert_eq!(snapshot.data.subagent.profiles.len(), 3);
    assert!(snapshot.data.subagent.profiles.iter().any(|p| p.id == "pi/search-cheap"));
    assert!(snapshot.data.subagent.profiles.iter().all(|p| p.is_builtin));

    // Create a user profile.
    let created = sup
        .profile_create(ProfileCreateRequestDto {
            id: "my-reviewer".to_string(),
            model: "deepseek-chat".to_string(),
            provider_id: None,
            tools: Some(vec!["read".to_string(), "grep".to_string()]),
            context_budget_tokens: Some(4000),
            timeout_seconds: Some(150),
            write_access: Some(false),
        })
        .await
        .unwrap();
    assert_eq!(created.id, "my-reviewer");
    assert!(!created.is_builtin);
    assert_eq!(created.model, "deepseek-chat");
    assert_eq!(created.context_budget_tokens, 4000);

    // Settings snapshot now shows 4 profiles.
    let snapshot = sup.settings_snapshot().await.unwrap();
    assert_eq!(snapshot.data.subagent.profiles.len(), 4);

    // Update model + provider_id (patch semantics).
    let updated = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "my-reviewer".to_string(),
            provider_id: Some("".to_string()), // unbind (empty string = None)
            model: Some("qwen-coder".to_string()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap();
    assert_eq!(updated.model, "qwen-coder");
    // provider_id was Some("") → unbound → None.
    assert_eq!(updated.provider_id, None);
    // tools/context_budget_tokens unchanged (patch semantics).
    assert_eq!(updated.tools, vec!["read", "grep"]);
    assert_eq!(updated.context_budget_tokens, 4000);

    // Delete the user profile.
    sup.profile_delete(ProfileDeleteRequestDto {
        id: "my-reviewer".to_string(),
    })
    .await
    .unwrap();
    let snapshot = sup.settings_snapshot().await.unwrap();
    assert_eq!(snapshot.data.subagent.profiles.len(), 3);

    sup.shutdown_writer().await.unwrap();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p busytok-runtime --test supervisor_control profile_crud_round_trips 2>&1 | tail -10`
Expected: FAIL — `ProfileCreateRequestDto` not found, `profile_create` method not found, `SettingsSnapshotDto.subagent` field missing.

- [ ] **Step 3: Implement `profile_to_dto` free function**

In `crates/busytok-runtime/src/supervisor.rs`, after `profile_provider_id` (line ~5732), add:

```rust
/// Maps a `SubagentProfileConfig` (settings-layer type) to a `ProfileDto`
/// (wire type). Mirrors `provider_to_dto` pattern.
///
/// `is_builtin` is derived from the profile name via `is_builtin_profile()`
/// — not stored in config. This is the single mapping point; both
/// `settings_snapshot` and `profile_create`/`profile_update` use it.
///
/// `provider_id` is extracted via the existing `profile_provider_id()`
/// helper (supervisor.rs:5719) — the single source of truth for provider
/// extraction, already used by `provider_delete`'s reference check.
fn profile_to_dto(
    name: &str,
    profile: &busytok_config::SubagentProfileConfig,
) -> ProfileDto {
    ProfileDto {
        id: name.to_string(),
        is_builtin: busytok_config::is_builtin_profile(name),
        provider_id: profile_provider_id(profile),
        model: profile.model.clone(),
        tools: profile.tools.clone(),
        context_budget_tokens: profile.context_budget_tokens,
        timeout_seconds: profile.timeout_seconds,
        write_access: profile.write_access,
    }
}
```

- [ ] **Step 4: Extend `settings_snapshot` to include `subagent` section**

In the `settings_snapshot` handler (line ~4159), add `subagent` to the `SettingsSnapshotDto` literal (after `prompt_palette_default_action`):

```rust
                subagent: {
                    let profiles: Vec<ProfileDto> = settings
                        .subagent
                        .profiles
                        .iter()
                        .map(|(name, cfg)| profile_to_dto(name, cfg))
                        .collect();
                    SettingsSubagentDto {
                        enabled: settings.subagent.enabled,
                        profiles,
                    }
                },
```

No import addition needed — `supervisor.rs:28` already has `use busytok_protocol::dto::*;` (glob import), so the new `Profile*Dto` and `SettingsSubagentDto` types from Task 2 are automatically in scope.

- [ ] **Step 5: Implement `profile_create` handler**

In `crates/busytok-runtime/src/supervisor.rs`, after `provider_test_connection` (before `provider_to_dto`), add:

```rust
    async fn profile_create(&self, req: ProfileCreateRequestDto) -> Result<ProfileDto> {
        // Reject built-in profile names — they're reserved.
        if busytok_config::is_builtin_profile(&req.id) {
            tracing::warn!(event_code = "profile.create.rejected", profile_id = %req.id, reason = "reserved_name", "name is reserved for built-in profiles");
            anyhow::bail!("cannot create profile '{}': name is reserved for built-in profiles", req.id);
        }
        if req.id.is_empty() {
            tracing::warn!(event_code = "profile.create.rejected", reason = "empty_id", "profile id must not be empty");
            anyhow::bail!("profile id must not be empty");
        }
        // Validate id format: [a-z0-9/_-]+ (allows namespacing like "pi/my-profile").
        if !req.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '/' || c == '_' || c == '-') {
            tracing::warn!(event_code = "profile.create.rejected", profile_id = %req.id, reason = "invalid_id_format", "profile id must contain only [a-z0-9/_-]+");
            anyhow::bail!("profile id must contain only [a-z0-9/_-]+");
        }

        let mut pending = {
            let settings = self.settings.lock().unwrap();
            settings.clone()
        };
        if pending.subagent.profiles.contains_key(&req.id) {
            tracing::warn!(event_code = "profile.create.rejected", profile_id = %req.id, reason = "already_exists", "profile already exists");
            anyhow::bail!("profile already exists: {}", req.id);
        }

        // If provider_id is specified, validate the provider exists and is enabled.
        if let Some(ref pid) = req.provider_id {
            let provider = pending.providers.iter().find(|p| &p.id == pid.as_str())
                .ok_or_else(|| {
                    tracing::warn!(event_code = "profile.create.rejected", profile_id = %req.id, provider_id = %pid, reason = "provider_not_found", "provider not found");
                    anyhow::anyhow!("provider not found: {}", pid)
                })?;
            if !provider.enabled {
                tracing::warn!(event_code = "profile.create.rejected", profile_id = %req.id, provider_id = %pid, reason = "disabled_provider", "cannot bind to disabled provider");
                anyhow::bail!("cannot bind profile to disabled provider: {}", pid);
            }
            // Validate model is in provider's whitelist.
            if !provider.models.contains(&req.model) {
                tracing::warn!(event_code = "profile.create.rejected", profile_id = %req.id, provider_id = %pid, model = %req.model, reason = "model_not_in_whitelist", "model not in provider whitelist");
                anyhow::bail!(
                    "model '{}' is not in provider '{}'s model whitelist: {:?}",
                    req.model, pid, provider.models
                );
            }
        }

        let profile = busytok_config::SubagentProfileConfig {
            write_access: req.write_access.unwrap_or(false),
            tools: req.tools.unwrap_or_default(),
            model: req.model,
            provider_id: req.provider_id,
            context_budget_tokens: req.context_budget_tokens.unwrap_or(3000),
            timeout_seconds: req.timeout_seconds.unwrap_or(120),
        };

        pending.subagent.profiles.insert(req.id.clone(), profile.clone());
        pending.save(&self.paths)?;
        {
            let mut settings = self.settings.lock().unwrap();
            *settings = pending;
        }

        let dto = profile_to_dto(&req.id, &profile);
        tracing::info!(event_code = "profile.created", profile_id = %req.id, "profile created");
        Ok(dto)
    }
```

- [ ] **Step 6: Implement `profile_update` handler (patch semantics)**

After `profile_create`, add:

```rust
    async fn profile_update(&self, req: ProfileUpdateRequestDto) -> Result<ProfileDto> {
        let mut pending = {
            let settings = self.settings.lock().unwrap();
            settings.clone()
        };
        let profile = pending.subagent.profiles.get_mut(&req.id)
            .ok_or_else(|| {
                tracing::warn!(event_code = "profile.update.rejected", profile_id = %req.id, reason = "not_found", "profile not found");
                anyhow::anyhow!("profile not found: {}", req.id)
            })?;

        // Handle provider_id patch: Some("") = unbind, Some("x") = bind, None = unchanged.
        if let Some(ref pid) = req.provider_id {
            let new_provider_id = if pid.is_empty() {
                None
            } else {
                // Validate the new provider exists and is enabled.
                let provider = pending.providers.iter().find(|p| &p.id == pid.as_str())
                    .ok_or_else(|| {
                        tracing::warn!(event_code = "profile.update.rejected", profile_id = %req.id, provider_id = %pid, reason = "provider_not_found", "provider not found");
                        anyhow::anyhow!("provider not found: {}", pid)
                    })?;
                if !provider.enabled {
                    tracing::warn!(event_code = "profile.update.rejected", profile_id = %req.id, provider_id = %pid, reason = "disabled_provider", "cannot bind to disabled provider");
                    anyhow::bail!("cannot bind profile to disabled provider: {}", pid);
                }
                Some(pid.clone())
            };
            // If binding to a new provider, validate the CURRENT model is in the
            // new provider's whitelist (unless model is also being updated this call).
            if let Some(ref new_pid) = new_provider_id {
                let provider = pending.providers.iter().find(|p| &p.id == new_pid.as_str()).unwrap();
                let effective_model = req.model.as_ref().unwrap_or(&profile.model);
                if !provider.models.contains(effective_model) {
                    tracing::warn!(event_code = "profile.update.rejected", profile_id = %req.id, provider_id = %new_pid, model = %effective_model, reason = "model_not_in_whitelist", "model not in provider whitelist");
                    anyhow::bail!(
                        "model '{}' is not in provider '{}'s model whitelist: {:?}",
                        effective_model, new_pid, provider.models
                    );
                }
            }
            profile.provider_id = new_provider_id;
        }

        // Handle model patch: if updating model and provider is bound, validate.
        if let Some(ref model) = req.model {
            if let Some(ref pid) = profile.provider_id {
                let provider = pending.providers.iter().find(|p| &p.id == pid.as_str());
                if let Some(provider) = provider {
                    if !provider.models.contains(model) {
                        tracing::warn!(event_code = "profile.update.rejected", profile_id = %req.id, provider_id = %pid, model = %model, reason = "model_not_in_whitelist", "model not in provider whitelist");
                        anyhow::bail!(
                            "model '{}' is not in provider '{}'s model whitelist: {:?}",
                            model, pid, provider.models
                        );
                    }
                }
            }
            profile.model = model.clone();
        }

        if let Some(tools) = req.tools { profile.tools = tools; }
        if let Some(budget) = req.context_budget_tokens { profile.context_budget_tokens = budget; }
        if let Some(timeout) = req.timeout_seconds { profile.timeout_seconds = timeout; }
        if let Some(write_access) = req.write_access { profile.write_access = write_access; }

        let profile_snapshot = profile.clone();
        pending.save(&self.paths)?;
        {
            let mut settings = self.settings.lock().unwrap();
            *settings = pending;
        }

        let dto = profile_to_dto(&req.id, &profile_snapshot);
        tracing::info!(event_code = "profile.updated", profile_id = %req.id, "profile updated");
        Ok(dto)
    }
```

- [ ] **Step 7: Implement `profile_delete` handler**

After `profile_update`, add:

```rust
    async fn profile_delete(&self, req: ProfileDeleteRequestDto) -> Result<()> {
        // Reject deletion of built-in profiles.
        if busytok_config::is_builtin_profile(&req.id) {
            tracing::warn!(event_code = "profile.delete.rejected", profile_id = %req.id, reason = "builtin", "cannot delete built-in profile");
            anyhow::bail!("cannot delete built-in profile: {}", req.id);
        }

        let mut pending = {
            let settings = self.settings.lock().unwrap();
            settings.clone()
        };
        if !pending.subagent.profiles.contains_key(&req.id) {
            tracing::warn!(event_code = "profile.delete.rejected", profile_id = %req.id, reason = "not_found", "profile not found");
            anyhow::bail!("profile not found: {}", req.id);
        }

        pending.subagent.profiles.remove(&req.id);
        pending.save(&self.paths)?;
        {
            let mut settings = self.settings.lock().unwrap();
            *settings = pending;
        }

        tracing::info!(event_code = "profile.deleted", profile_id = %req.id, "profile deleted");
        Ok(())
    }
```

- [ ] **Step 8: Run the CRUD test to verify it passes**

Run: `cargo test -p busytok-runtime --test supervisor_control profile_crud_round_trips 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 9: Write validation tests**

Add to `crates/busytok-runtime/tests/supervisor_control.rs`:

```rust
#[tokio::test]
async fn profile_create_rejects_builtin_name() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .profile_create(ProfileCreateRequestDto {
            id: "pi/search-cheap".to_string(),
            model: "deepseek-chat".to_string(),
            provider_id: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("reserved for built-in"),
        "expected reserved-name error, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_delete_rejects_builtin() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .profile_delete(ProfileDeleteRequestDto {
            id: "pi/search-cheap".to_string(),
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("cannot delete built-in"),
        "expected built-in rejection, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_rejects_disabled_provider() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create a disabled provider.
    sup.provider_create(provider_create_request("disabled-p", "Disabled"))
        .await
        .unwrap();
    sup.provider_update(ProviderUpdateRequestDto {
        id: "disabled-p".to_string(),
        name: None,
        base_url: None,
        api_key_env_name: None,
        base_url_env_name: None,
        models: Some(vec!["some-model".to_string()]),
        enabled: Some(false),
        api_key: None,
    })
    .await
    .unwrap();

    // Try to bind a profile to the disabled provider → rejected.
    let err = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "pi/search-cheap".to_string(),
            provider_id: Some("disabled-p".to_string()),
            model: Some("some-model".to_string()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("disabled provider"),
        "expected disabled-provider rejection, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_rejects_stale_model_on_rebind() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create a provider with model "model-a".
    sup.provider_create(ProviderCreateRequestDto {
        id: "test-p".to_string(),
        name: "Test".to_string(),
        base_url: "https://api.test.com/v1".to_string(),
        api_key_env_name: "TEST_KEY".to_string(),
        base_url_env_name: None,
        models: vec!["model-a".to_string()],
        api_key: None,
    })
    .await
    .unwrap();

    // Bind profile to provider with model-a.
    sup.profile_update(ProfileUpdateRequestDto {
        id: "pi/search-cheap".to_string(),
        provider_id: Some("test-p".to_string()),
        model: Some("model-a".to_string()),
        tools: None,
        context_budget_tokens: None,
        timeout_seconds: None,
        write_access: None,
    })
    .await
    .unwrap();

    // Provider updates model list to ["model-b"] only — model-a is now stale.
    sup.provider_update(ProviderUpdateRequestDto {
        id: "test-p".to_string(),
        name: None,
        base_url: None,
        api_key_env_name: None,
        base_url_env_name: None,
        models: Some(vec!["model-b".to_string()]),
        enabled: None,
        api_key: None,
    })
    .await
    .unwrap();

    // Re-bind to the same provider (provider_id = Some("test-p")) without
    // changing the model → the rebind path validates the effective model
    // (model-a) against the new whitelist (["model-b"]) and rejects.
    let err = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "pi/search-cheap".to_string(),
            provider_id: Some("test-p".to_string()), // re-bind same provider
            model: None, // not changing model
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not in provider") || err.to_string().contains("whitelist"),
        "expected stale-model rejection, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_patches_tools_without_triggering_stale_check() {
    // Patching only `tools` (neither provider_id nor model) must NOT run
    // the whitelist validation — the service trusts the existing binding
    // and only the UI surfaces stale-model warnings for already-bound profiles.
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    sup.provider_create(ProviderCreateRequestDto {
        id: "test-p".to_string(),
        name: "Test".to_string(),
        base_url: "https://api.test.com/v1".to_string(),
        api_key_env_name: "TEST_KEY".to_string(),
        base_url_env_name: None,
        models: vec!["model-a".to_string()],
        api_key: None,
    })
    .await
    .unwrap();
    sup.profile_update(ProfileUpdateRequestDto {
        id: "pi/search-cheap".to_string(),
        provider_id: Some("test-p".to_string()),
        model: Some("model-a".to_string()),
        tools: None,
        context_budget_tokens: None,
        timeout_seconds: None,
        write_access: None,
    })
    .await
    .unwrap();

    // Shrink the provider's whitelist so model-a is no longer in it.
    sup.provider_update(ProviderUpdateRequestDto {
        id: "test-p".to_string(),
        name: None,
        base_url: None,
        api_key_env_name: None,
        base_url_env_name: None,
        models: Some(vec!["model-b".to_string()]),
        enabled: None,
        api_key: None,
    })
    .await
    .unwrap();

    // Patch ONLY tools — should succeed despite the stale model, because
    // the service does not re-validate existing bindings on unrelated patches.
    let updated = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "pi/search-cheap".to_string(),
            provider_id: None, // unchanged
            model: None, // unchanged
            tools: Some(vec!["new-tool".to_string()]),
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap();
    assert_eq!(updated.tools, vec!["new-tool".to_string()]);
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn settings_snapshot_includes_subagent_profiles() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let snapshot = sup.settings_snapshot().await.unwrap();
    assert!(snapshot.data.subagent.enabled);
    assert_eq!(snapshot.data.subagent.profiles.len(), 3);
    let search = snapshot.data.subagent.profiles.iter()
        .find(|p| p.id == "pi/search-cheap")
        .unwrap();
    assert!(search.is_builtin);
    assert_eq!(search.model, "deepseek-chat");
    assert_eq!(search.provider_id, None);
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_create_rejects_nonexistent_provider() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .profile_create(ProfileCreateRequestDto {
            id: "my-profile".to_string(),
            model: "some-model".to_string(),
            provider_id: Some("nonexistent-provider".to_string()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("provider not found"),
        "expected provider-not-found error, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_create_rejects_model_not_in_whitelist() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create a provider with only "model-a".
    sup.provider_create(ProviderCreateRequestDto {
        id: "test-p".to_string(),
        name: "Test".to_string(),
        base_url: "https://api.test.com/v1".to_string(),
        api_key_env_name: "TEST_KEY".to_string(),
        base_url_env_name: None,
        models: vec!["model-a".to_string()],
        api_key: None,
    })
    .await
    .unwrap();

    // Try to create a profile bound to that provider with a model NOT in its whitelist.
    let err = sup
        .profile_create(ProfileCreateRequestDto {
            id: "my-profile".to_string(),
            model: "model-b".to_string(),
            provider_id: Some("test-p".to_string()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not in provider") || err.to_string().contains("whitelist"),
        "expected whitelist rejection, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_rejects_nonexistent_profile() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "nonexistent-profile".to_string(),
            provider_id: None,
            model: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("profile not found"),
        "expected not-found error, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_delete_rejects_nonexistent_profile() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .profile_delete(ProfileDeleteRequestDto {
            id: "nonexistent-profile".to_string(),
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("profile not found"),
        "expected not-found error, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_create_rejects_duplicate_id() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // First create succeeds.
    sup.profile_create(ProfileCreateRequestDto {
        id: "my-profile".to_string(),
        model: "some-model".to_string(),
        provider_id: None,
        tools: None,
        context_budget_tokens: None,
        timeout_seconds: None,
        write_access: None,
    })
    .await
    .unwrap();

    // Second create with the same id fails.
    let err = sup
        .profile_create(ProfileCreateRequestDto {
            id: "my-profile".to_string(),
            model: "other-model".to_string(),
            provider_id: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("already exists"),
        "expected already-exists error, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_unbinds_provider_with_empty_string() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create a provider + bind a user profile to it.
    sup.provider_create(ProviderCreateRequestDto {
        id: "test-p".to_string(),
        name: "Test".to_string(),
        base_url: "https://api.test.com/v1".to_string(),
        api_key_env_name: "TEST_KEY".to_string(),
        base_url_env_name: None,
        models: vec!["model-a".to_string()],
        api_key: None,
    })
    .await
    .unwrap();
    sup.profile_create(ProfileCreateRequestDto {
        id: "my-profile".to_string(),
        model: "model-a".to_string(),
        provider_id: Some("test-p".to_string()),
        tools: None,
        context_budget_tokens: None,
        timeout_seconds: None,
        write_access: None,
    })
    .await
    .unwrap();

    // Unbind via Some("").
    let updated = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "my-profile".to_string(),
            provider_id: Some("".to_string()),
            model: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap();
    assert_eq!(updated.provider_id, None);
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_with_all_none_patch_is_noop() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Capture the built-in search profile's pre-state.
    let before = sup.settings_snapshot().await.unwrap().data.subagent.profiles.iter()
        .find(|p| p.id == "pi/search-cheap")
        .cloned()
        .unwrap();

    // Patch everything as None (no-op).
    let after = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "pi/search-cheap".to_string(),
            provider_id: None,
            model: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap();

    // Profile is returned unchanged.
    assert_eq!(after.model, before.model);
    assert_eq!(after.provider_id, before.provider_id);
    assert_eq!(after.tools, before.tools);
    assert_eq!(after.context_budget_tokens, before.context_budget_tokens);
    assert_eq!(after.timeout_seconds, before.timeout_seconds);
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_changes_provider_and_model_together() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create two providers, each with a different model.
    sup.provider_create(ProviderCreateRequestDto {
        id: "prov-a".to_string(),
        name: "Prov A".to_string(),
        base_url: "https://a.test/v1".to_string(),
        api_key_env_name: "A_KEY".to_string(),
        base_url_env_name: None,
        models: vec!["model-a".to_string()],
        api_key: None,
    })
    .await
    .unwrap();
    sup.provider_create(ProviderCreateRequestDto {
        id: "prov-b".to_string(),
        name: "Prov B".to_string(),
        base_url: "https://b.test/v1".to_string(),
        api_key_env_name: "B_KEY".to_string(),
        base_url_env_name: None,
        models: vec!["model-b".to_string()],
        api_key: None,
    })
    .await
    .unwrap();
    // Create a profile bound to prov-a/model-a.
    sup.profile_create(ProfileCreateRequestDto {
        id: "my-profile".to_string(),
        model: "model-a".to_string(),
        provider_id: Some("prov-a".to_string()),
        tools: None,
        context_budget_tokens: None,
        timeout_seconds: None,
        write_access: None,
    })
    .await
    .unwrap();

    // Atomically switch to prov-b/model-b.
    let updated = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "my-profile".to_string(),
            provider_id: Some("prov-b".to_string()),
            model: Some("model-b".to_string()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap();
    assert_eq!(updated.provider_id, Some("prov-b".to_string()));
    assert_eq!(updated.model, "model-b");
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_create_rejects_invalid_id_format() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Uppercase and spaces are not allowed.
    let err = sup
        .profile_create(ProfileCreateRequestDto {
            id: "My Profile".to_string(),
            model: "some-model".to_string(),
            provider_id: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("[a-z0-9/_-]+") || err.to_string().contains("id format"),
        "expected id-format rejection, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_patches_only_model_on_bound_profile() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create provider + profile bound to it.
    sup.provider_create(ProviderCreateRequestDto {
        id: "test-p".to_string(),
        name: "Test".to_string(),
        base_url: "https://api.test.com/v1".to_string(),
        api_key_env_name: "TEST_KEY".to_string(),
        base_url_env_name: None,
        models: vec!["model-a".to_string(), "model-b".to_string()],
        api_key: None,
    })
    .await
    .unwrap();
    sup.profile_create(ProfileCreateRequestDto {
        id: "my-profile".to_string(),
        model: "model-a".to_string(),
        provider_id: Some("test-p".to_string()),
        tools: None,
        context_budget_tokens: None,
        timeout_seconds: None,
        write_access: None,
    })
    .await
    .unwrap();

    // Patch only the model (provider_id stays None = unchanged).
    let updated = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "my-profile".to_string(),
            provider_id: None,
            model: Some("model-b".to_string()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap();
    // Provider unchanged, model updated.
    assert_eq!(updated.provider_id, Some("test-p".to_string()));
    assert_eq!(updated.model, "model-b");
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_create_applies_defaults_for_omitted_fields() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create a profile with tools/budget/timeout all None.
    let dto = sup
        .profile_create(ProfileCreateRequestDto {
            id: "my-profile".to_string(),
            model: "some-model".to_string(),
            provider_id: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap();

    // Defaults: write_access=false, tools=[], budget=3000, timeout=120.
    assert_eq!(dto.write_access, false);
    assert_eq!(dto.tools, Vec::<String>::new());
    assert_eq!(dto.context_budget_tokens, 3000);
    assert_eq!(dto.timeout_seconds, 120);
    sup.shutdown_writer().await.unwrap();
}
```

- [ ] **Step 10: Run all validation tests**

Run: `cargo test -p busytok-runtime --test supervisor_control profile_ 2>&1 | tail -15`
Expected: All profile tests PASS.

- [ ] **Step 11: Run full test suite to check for regressions**

Run: `cargo test -p busytok-runtime --test supervisor_control 2>&1 | tail -5`
Expected: All tests PASS (108 prior + new profile tests).

- [ ] **Step 12: Commit**

```bash
git add crates/busytok-runtime/src/supervisor.rs crates/busytok-runtime/tests/supervisor_control.rs
git commit -m "feat(runtime): implement profile CRUD handlers + settings_snapshot extension

Phase 4 Task 4: profile_create (rejects built-in names, validates
disabled provider + stale model), profile_update (patch semantics:
Some(\"\") = unbind provider_id, None = unchanged), profile_delete
(rejects built-in). settings_snapshot now includes subagent: { enabled,
profiles[] }. Reuses profile_provider_id() helper and provider_to_dto
pattern. All handlers log with event_code pattern."
```

---

### Task 5: Frontend — Client Methods + Hooks

**Files:**
- Modify: `apps/gui/src/api/busytokClient.ts`
- Modify: `apps/gui/src/api/useBusytokData.ts`

**Interfaces:**
- Produces: `profileCreate`, `profileUpdate`, `profileDelete` methods on `BusytokClient`.
- Produces: `useProfileMutations` hook (returns `{ createProfile, updateProfile, deleteProfile }`).
- Consumes: `ProfileDto`, `ProfileCreateRequestDto`, `ProfileUpdateRequestDto`, `ProfileDeleteRequestDto` from `@busytok/protocol-types`.

- [ ] **Step 1: Add client methods**

In `apps/gui/src/api/busytokClient.ts`, after the provider methods (after line ~183), add:

```ts
    // Profiles (Phase 4) — bare DTOs (not envelope-wrapped), mirroring provider pattern.
    // Read path: profiles come via settings.snapshot (subagent.profiles[]).
    // Write path: dedicated profile.* RPCs.
    profileCreate: (request: ProfileCreateRequestDto) =>
      call<ProfileDto>("profile.create", { ...request }),
    profileUpdate: (request: ProfileUpdateRequestDto) =>
      call<ProfileDto>("profile.update", { ...request }),
    profileDelete: (id: string) =>
      call<void>("profile.delete", { id }),
```

Also add `ProfileCreateRequestDto`, `ProfileDto`, `ProfileUpdateRequestDto` to the existing `import type { ... } from "@busytok/protocol-types"` block at the top of the file (after the existing `ProviderUpdateRequestDto` import).

- [ ] **Step 2: Add hooks**

In `apps/gui/src/api/useBusytokData.ts`, after the provider hooks (after line ~402), add:

```ts
// ── Profiles (Phase 4) ───────────────────────────────────────────────

/**
 * Profile mutations. All three invalidate `settingsSnapshot` on success
 * because profiles are READ via settings.snapshot (not a dedicated
 * profile.list RPC). This keeps the read+write paths consistent.
 */
export function useProfileMutations() {
  const client = useBusytokClient();
  const queryClient = useQueryClient();
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: queryKeys.settingsSnapshot() });

  const createProfile = useMutation({
    mutationFn: (req: ProfileCreateRequestDto) => client.profileCreate(req),
    onSuccess: invalidate,
  });
  const updateProfile = useMutation({
    mutationFn: (req: ProfileUpdateRequestDto) => client.profileUpdate(req),
    onSuccess: invalidate,
  });
  const deleteProfile = useMutation({
    mutationFn: (id: string) => client.profileDelete(id),
    onSuccess: invalidate,
  });

  return { createProfile, updateProfile, deleteProfile };
}
```

Add `ProfileCreateRequestDto` and `ProfileUpdateRequestDto` to the existing `import type { ... } from "@busytok/protocol-types"` block at the top of the file (after the existing `ProviderUpdateRequestDto` import).

- [ ] **Step 3: Regenerate ts-rs types + patch GUI `SettingsSnapshotDto` fixtures**

Task 2 added a required `subagent` field to `SettingsSnapshotDto`. The TypeScript types must be regenerated, and two GUI test fixtures that construct `SettingsSnapshotDto` literals must be patched before `tsc --noEmit` will pass.

First, regenerate the TypeScript types from Rust:

```bash
cargo test -p busytok-protocol generate_typescript_types
```

Then patch `apps/gui/src/App.test.tsx` — in the `settingsSnapshot` mock at **line ~256**, add the `subagent` field before the closing `}`:

```ts
      recovery_actions: [],
      prompt_palette_default_action: "OnlyCopy",
      subagent: {
        enabled: true,
        profiles: [],
      },
    }),
```

Then patch `apps/gui/src/pages/SettingsPageCoverage.test.tsx` — in the `snapshot()` factory at **line ~112**, add the `subagent` field before `...overrides`:

```ts
    prompt_palette_default_action: "OnlyCopy",
    recovery_actions: [],
    diagnostics: diagnostics(),
    subagent: {
      enabled: true,
      profiles: [],
    },
    ...overrides,
```

- [ ] **Step 4: Typecheck**

Run: `cd apps/gui && pnpm tsc --noEmit 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/gui/src/api/busytokClient.ts apps/gui/src/api/useBusytokData.ts apps/gui/src/App.test.tsx apps/gui/src/pages/SettingsPageCoverage.test.tsx
git commit -m "feat(gui): add profile client methods + hooks; patch SettingsSnapshotDto fixtures

Phase 4 Task 5: profileCreate/Update/Delete client methods + useProfileMutations
hook. All mutations invalidate settingsSnapshot because profiles are read
via settings.snapshot (spec: 'Read: settings.snapshot returns
subagent.profiles[]'). Patches App.test.tsx and SettingsPageCoverage.test.tsx
fixtures with the new required subagent field."
```

---

### Task 6: Frontend — ProfilesSection Component + Tests (TDD)

**Files:**
- Create: `apps/gui/src/components/ProfilesSection.tsx`
- Create: `apps/gui/src/components/ProfilesSection.test.tsx`
- Modify: `apps/gui/src/pages/ProvidersPage.tsx`
- Modify: `apps/gui/src/pages/ProvidersPage.test.tsx`

**Interfaces:**
- Produces: `ProfilesSection` React component (rendered inside ProvidersPage).
- Consumes: `useSettingsSnapshot()` for profile data (read path).
- Consumes: `useProviders()` for the provider dropdown.
- Consumes: `useProfileMutations()` for create/update/delete.
- Consumes: `ProfileDto`, `ProviderDto` from `@busytok/protocol-types`.

- [ ] **Step 1: Write failing test for ProfilesSection**

Create `apps/gui/src/components/ProfilesSection.test.tsx`:

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type {
  ProviderDto,
  ProviderListResponseDto,
  ProfileDto,
  ReadEnvelopeDto,
  SettingsSnapshotDto,
} from "@busytok/protocol-types";

vi.mock("../api/useBusytokData", () => ({
  useSettingsSnapshot: vi.fn(),
  useProviders: vi.fn(),
  useProfileMutations: vi.fn(),
}));
vi.mock("../logging/safeReporter", () => ({
  reportFrontendEventSafely: vi.fn(),
}));

import { useSettingsSnapshot, useProviders, useProfileMutations } from "../api/useBusytokData";
import { ProfilesSection } from "./ProfilesSection";

const mockSnapshot = vi.mocked(useSettingsSnapshot);
const mockProviders = vi.mocked(useProviders);
const mockMutations = vi.mocked(useProfileMutations);

function makeProfile(overrides: Partial<ProfileDto> = {}): ProfileDto {
  return {
    id: "pi/search-cheap",
    is_builtin: true,
    provider_id: null,
    model: "deepseek-chat",
    tools: ["read", "grep"],
    context_budget_tokens: 3000,
    timeout_seconds: 120,
    write_access: false,
    ...overrides,
  };
}

function makeProvider(overrides: Partial<ProviderDto> = {}): ProviderDto {
  return {
    id: "deepseek",
    name: "DeepSeek",
    base_url: "https://api.deepseek.com/v1",
    api_key_env_name: "DEEPSEEK_API_KEY",
    base_url_env_name: null,
    models: ["deepseek-chat"],
    enabled: true,
    has_api_key: true,
    ...overrides,
  };
}

function renderWithProviders(ui: React.ReactElement) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>{ui}</QueryClientProvider>,
  );
}

describe("ProfilesSection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders built-in profiles from settings snapshot", () => {
    const profiles = [
      makeProfile({ id: "pi/search-cheap", is_builtin: true }),
      makeProfile({ id: "pi/review-cheap", is_builtin: true, model: "qwen-coder" }),
    ];
    mockSnapshot.mockReturnValue({
      data: { data: { subagent: { enabled: true, profiles } } } as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText("pi/search-cheap")).toBeInTheDocument();
    expect(screen.getByText("pi/review-cheap")).toBeInTheDocument();
  });

  it("shows ⚠ warning when profile is bound to a disabled provider", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: "disabled-p" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "disabled-p", enabled: false })],
      } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/disabled provider/i)).toBeInTheDocument();
  });

  it("shows stale model warning when model not in provider whitelist", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: "deepseek", model: "stale-model" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "deepseek", models: ["deepseek-chat"] })],
      } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/stale|invalid model/i)).toBeInTheDocument();
  });

  it("calls profileUpdate when binding provider + model", async () => {
    const updateMutate = vi.fn();
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "deepseek-chat" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "deepseek", models: ["deepseek-chat"] })],
      } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: updateMutate, isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);

    // Click "Edit" on the profile row.
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));

    // Select provider from dropdown.
    const select = screen.getByLabelText(/provider/i);
    fireEvent.change(select, { target: { value: "deepseek" } });

    // Click Save.
    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(updateMutate).toHaveBeenCalledWith(
        expect.objectContaining({
          id: "pi/search-cheap",
          provider_id: "deepseek",
          model: "deepseek-chat",
        }),
      );
    });
  });

  it("hides the Delete button for built-in profiles", () => {
    const profiles = [
      makeProfile({ id: "pi/search-cheap", is_builtin: true }),
      makeProfile({ id: "my-custom", is_builtin: false }),
    ];
    mockSnapshot.mockReturnValue({
      data: { data: { subagent: { enabled: true, profiles } } } as ReadEnvelopeDto<SettingsSnapshotDto>,
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    // Built-in profile row: no Delete button.
    const builtinRow = screen.getByText("pi/search-cheap").closest(".settings-panel");
    expect(builtinRow?.querySelector('button[class*="btn--danger"]')).toBeNull();
    // User profile row: Delete button present.
    const userRow = screen.getByText("my-custom").closest(".settings-panel");
    expect(userRow?.querySelector('button[class*="btn--danger"]')).not.toBeNull();
  });

  it("calls deleteProfile.mutate when Delete is clicked on a user profile", () => {
    const deleteMutate = vi.fn();
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ id: "my-custom", is_builtin: false })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: deleteMutate, isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /delete/i }));
    expect(deleteMutate).toHaveBeenCalledWith("my-custom");
  });

  it("cascade-filters the model dropdown when provider changes", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [
          makeProvider({ id: "deepseek", models: ["deepseek-chat", "deepseek-reasoner"] }),
          makeProvider({ id: "openai", name: "OpenAI", models: ["gpt-4", "gpt-3.5-turbo"] }),
        ],
      } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));

    // Select deepseek → model dropdown shows deepseek models.
    fireEvent.change(screen.getByLabelText(/provider/i), { target: { value: "deepseek" } });
    let modelSelect = screen.getByLabelText(/model/i) as HTMLSelectElement;
    expect(modelSelect.innerHTML).toContain("deepseek-chat");
    expect(modelSelect.innerHTML).toContain("deepseek-reasoner");
    expect(modelSelect.innerHTML).not.toContain("gpt-4");

    // Switch to openai → model dropdown now shows openai models only.
    fireEvent.change(screen.getByLabelText(/provider/i), { target: { value: "openai" } });
    modelSelect = screen.getByLabelText(/model/i) as HTMLSelectElement;
    expect(modelSelect.innerHTML).toContain("gpt-4");
    expect(modelSelect.innerHTML).not.toContain("deepseek-chat");
  });

  it("Cancel button exits edit mode without calling mutate", () => {
    const updateMutate = vi.fn();
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "deepseek-chat" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: updateMutate, isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    // No Save button after cancel (back to view mode).
    expect(screen.queryByRole("button", { name: /save/i })).toBeNull();
    // updateProfile was NOT called.
    expect(updateMutate).not.toHaveBeenCalled();
  });

  it("disables Save button when model is not in selected provider's whitelist", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "deepseek-chat" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: {
        providers: [makeProvider({ id: "deepseek", models: ["deepseek-chat"] })],
      } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    // Select provider — model auto-resets to first available ("deepseek-chat"),
    // so Save is enabled.
    fireEvent.change(screen.getByLabelText(/provider/i), { target: { value: "deepseek" } });
    let saveBtn = screen.getByRole("button", { name: /save/i }) as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(false);

    // Manually clear the model selection (set to empty option) → Save disabled.
    fireEvent.change(screen.getByLabelText(/model/i), { target: { value: "" } });
    saveBtn = screen.getByRole("button", { name: /save/i }) as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(true);
  });

  it("shows loading state when snapshot is loading", () => {
    mockSnapshot.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/loading profiles/i)).toBeInTheDocument();
  });

  it("shows error state when snapshot fetch fails", () => {
    mockSnapshot.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/failed to load profiles/i)).toBeInTheDocument();
  });

  it("renders empty state when no profiles configured", () => {
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    expect(screen.getByText(/no profiles configured/i)).toBeInTheDocument();
  });

  it("shows mutation error when updateProfile fails", async () => {
    const updateMutate = vi.fn(
      (_payload: unknown, opts?: { onError?: (err: Error) => void }) => {
        opts?.onError?.(new Error("model not in provider whitelist"));
      },
    );
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "deepseek-chat" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: updateMutate, isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(screen.getByText(/model not in provider whitelist/i)).toBeInTheDocument();
    });
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "profile.update.failed" }),
    );
  });

  it("shows mutation error when deleteProfile fails", async () => {
    const deleteMutate = vi.fn(
      (_id: string, opts?: { onError?: (err: Error) => void }) => {
        opts?.onError?.(new Error("cannot delete built-in profile"));
      },
    );
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ id: "my-profile", is_builtin: false })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: vi.fn(), isPending: false },
      deleteProfile: { mutate: deleteMutate, isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    fireEvent.click(screen.getByRole("button", { name: /delete/i }));
    await waitFor(() => {
      expect(screen.getByText(/cannot delete built-in profile/i)).toBeInTheDocument();
    });
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "profile.delete.failed" }),
    );
  });

  it("clears mutation error when starting a new edit", () => {
    const updateMutate = vi.fn(
      (_payload: unknown, opts?: { onError?: (err: Error) => void }) => {
        opts?.onError?.(new Error("first error"));
      },
    );
    mockSnapshot.mockReturnValue({
      data: {
        data: {
          subagent: {
            enabled: true,
            profiles: [makeProfile({ provider_id: null, model: "deepseek-chat" })],
          },
        } as ReadEnvelopeDto<SettingsSnapshotDto>,
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    } as never);
    mockProviders.mockReturnValue({
      data: { providers: [] } as ProviderListResponseDto,
      isLoading: false,
    } as never);
    mockMutations.mockReturnValue({
      createProfile: { mutate: vi.fn(), isPending: false },
      updateProfile: { mutate: updateMutate, isPending: false },
      deleteProfile: { mutate: vi.fn(), isPending: false },
    } as never);

    renderWithProviders(<ProfilesSection />);
    // Trigger first error.
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    expect(screen.getByText(/first error/i)).toBeInTheDocument();

    // Click Edit again — error should clear.
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    expect(screen.queryByText(/first error/i)).toBeNull();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/gui && pnpm vitest run src/components/ProfilesSection.test.tsx 2>&1 | tail -10`
Expected: FAIL — `ProfilesSection` module not found.

- [ ] **Step 3: Implement `ProfilesSection` component**

Create `apps/gui/src/components/ProfilesSection.tsx`:

```tsx
import { useCallback, useMemo, useState } from "react";
import type {
  ProfileDto,
  ProviderDto,
  ProfileUpdateRequestDto,
} from "@busytok/protocol-types";
import {
  useSettingsSnapshot,
  useProviders,
  useProfileMutations,
} from "../api/useBusytokData";
import { PageState } from "./PageState";
import { SettingsActionGroup } from "./desktop/SettingsActionGroup";
import { SettingsRow } from "./desktop/SettingsRow";
import { SettingsValue } from "./desktop/SettingsValue";
import { reportFrontendEventSafely } from "../logging/safeReporter";

// ── Helpers ──────────────────────────────────────────────────────────

/** Returns enabled providers for the binding dropdown (spec: "only enabled"). */
function enabledProviders(providers: ProviderDto[]): ProviderDto[] {
  return providers.filter((p) => p.enabled);
}

/** Returns true if the profile's model is NOT in the bound provider's whitelist. */
function isStaleModel(profile: ProfileDto, providers: ProviderDto[]): boolean {
  if (!profile.provider_id) return false;
  const provider = providers.find((p) => p.id === profile.provider_id);
  if (!provider) return true; // provider deleted → stale
  return !provider.models.includes(profile.model);
}

/** Returns true if the profile is bound to a disabled provider. */
function isBoundToDisabledProvider(
  profile: ProfileDto,
  providers: ProviderDto[],
): boolean {
  if (!profile.provider_id) return false;
  const provider = providers.find((p) => p.id === profile.provider_id);
  return provider != null && !provider.enabled;
}

// ── ProfileRow ───────────────────────────────────────────────────────

interface ProfileRowProps {
  profile: ProfileDto;
  providers: ProviderDto[];
  isEditing: boolean;
  editProviderId: string;
  editModel: string;
  onEdit: (profile: ProfileDto) => void;
  onEditChange: (patch: { providerId?: string; model?: string }) => void;
  onEditSubmit: () => void;
  onEditCancel: () => void;
  isEditPending: boolean;
  onDelete: (id: string) => void;
  isDeletePending: boolean;
}

function ProfileRow({
  profile,
  providers,
  isEditing,
  editProviderId,
  editModel,
  onEdit,
  onEditChange,
  onEditSubmit,
  onEditCancel,
  isEditPending,
  onDelete,
  isDeletePending,
}: ProfileRowProps) {
  const disabled = isBoundToDisabledProvider(profile, providers);
  const stale = isStaleModel(profile, providers);
  const enabledProvs = enabledProviders(providers);

  // Cascade-filtered models: only show models from the selected provider.
  const availableModels = useMemo(() => {
    const selected = enabledProvs.find((p) => p.id === editProviderId);
    return selected ? selected.models : [];
  }, [enabledProvs, editProviderId]);

  // Disable Save when a provider is selected but the model is not in its
  // whitelist (stale or unselected) — spec: "requires re-selection before save".
  const isEditModelStale =
    editProviderId !== "" && !availableModels.includes(editModel);

  return (
    <div className="settings-panel">
      <SettingsRow
        label={profile.id}
        description={profile.is_builtin ? "Built-in profile" : "User profile"}
        control={
          <SettingsValue
            value={profile.is_builtin ? "Built-in" : "Custom"}
            tone="muted"
          />
        }
      />
      {disabled && (
        <SettingsRow
          label="⚠ Warning"
          control={
            <SettingsValue
              value="Bound to a disabled provider — delegate will fail until rebound"
              tone="danger"
            />
          }
        />
      )}
      {stale && !isEditing && (
        <SettingsRow
          label="⚠ Stale Model"
          control={
            <SettingsValue
              value={`Model '${profile.model}' is not in the provider's whitelist — re-select before save`}
              tone="danger"
            />
          }
        />
      )}
      {isEditing ? (
        <>
          <SettingsRow
            layout="vertical"
            label="Provider"
            description="Only enabled providers can be selected."
            control={
              <select
                className="input"
                aria-label="Provider"
                value={editProviderId}
                onChange={(e) => onEditChange({ providerId: e.currentTarget.value })}
              >
                <option value="">— None (unbound) —</option>
                {enabledProvs.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name} ({p.id})
                  </option>
                ))}
              </select>
            }
          />
          <SettingsRow
            layout="vertical"
            label="Model"
            description="Models available from the selected provider."
            control={
              <select
                className="input"
                aria-label="Model"
                value={editModel}
                onChange={(e) => onEditChange({ model: e.currentTarget.value })}
                disabled={availableModels.length === 0}
              >
                <option value="">— Select model —</option>
                {availableModels.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            }
          />
          <SettingsRow
            label="Advanced (read-only)"
            control={
              <SettingsActionGroup direction="col">
                <SettingsValue value={`Tools: ${profile.tools.join(", ")}`} tone="muted" />
                <SettingsValue value={`Budget: ${profile.context_budget_tokens} tokens`} tone="muted" />
                <SettingsValue value={`Timeout: ${profile.timeout_seconds}s`} tone="muted" />
              </SettingsActionGroup>
            }
          />
          <SettingsRow
            label="Actions"
            control={
              <SettingsActionGroup direction="row">
                <button
                  type="button"
                  className="btn btn--primary btn--sm"
                  onClick={onEditSubmit}
                  disabled={isEditPending || isEditModelStale}
                >
                  {isEditPending ? "Saving..." : "Save"}
                </button>
                <button
                  type="button"
                  className="btn btn--secondary btn--sm"
                  onClick={onEditCancel}
                  disabled={isEditPending}
                >
                  Cancel
                </button>
              </SettingsActionGroup>
            }
          />
        </>
      ) : (
        <>
          <SettingsRow
            label="Provider"
            control={
              <SettingsValue
                value={profile.provider_id ?? "— unbound —"}
                tone={profile.provider_id ? "default" : "muted"}
              />
            }
          />
          <SettingsRow
            label="Model"
            control={
              <SettingsValue
                value={profile.model}
                tone={stale ? "danger" : "default"}
              />
            }
          />
          <SettingsRow
            label="Advanced (read-only)"
            control={
              <SettingsActionGroup direction="col">
                <SettingsValue value={`Tools: ${profile.tools.join(", ")}`} tone="muted" />
                <SettingsValue value={`Budget: ${profile.context_budget_tokens} tokens`} tone="muted" />
                <SettingsValue value={`Timeout: ${profile.timeout_seconds}s`} tone="muted" />
              </SettingsActionGroup>
            }
          />
          <SettingsRow
            label="Actions"
            control={
              <SettingsActionGroup direction="row">
                <button
                  type="button"
                  className="btn btn--primary btn--sm"
                  onClick={() => onEdit(profile)}
                >
                  Edit
                </button>
                {!profile.is_builtin && (
                  <button
                    type="button"
                    className="btn btn--danger btn--sm"
                    onClick={() => onDelete(profile.id)}
                    disabled={isDeletePending}
                  >
                    Delete
                  </button>
                )}
              </SettingsActionGroup>
            }
          />
        </>
      )}
    </div>
  );
}

// ── ProfilesSection ──────────────────────────────────────────────────

export function ProfilesSection() {
  const snapshotQuery = useSettingsSnapshot();
  const providersQuery = useProviders();
  const { updateProfile, deleteProfile } = useProfileMutations();

  const [editingId, setEditingId] = useState<string | null>(null);
  const [editProviderId, setEditProviderId] = useState("");
  const [editModel, setEditModel] = useState("");
  const [mutationError, setMutationError] = useState<string | null>(null);

  const profiles = snapshotQuery.data?.data?.subagent?.profiles ?? [];
  const providers = providersQuery.data?.providers ?? [];

  const handleEdit = useCallback((profile: ProfileDto) => {
    setEditingId(profile.id);
    setEditProviderId(profile.provider_id ?? "");
    setEditModel(profile.model);
    setMutationError(null);
  }, []);

  const handleEditChange = useCallback(
    (patch: { providerId?: string; model?: string }) => {
      if (patch.providerId !== undefined) {
        setEditProviderId(patch.providerId);
        // Cascade: when the provider changes, reset the model to the first
        // available model from the new provider (or empty if none).
        const newProvider = providers.find((p) => p.id === patch.providerId);
        setEditModel(newProvider?.models[0] ?? "");
      }
      if (patch.model !== undefined) {
        setEditModel(patch.model);
      }
    },
    [providers],
  );

  const handleEditSubmit = useCallback(() => {
    if (!editingId) return;
    setMutationError(null);
    const req: ProfileUpdateRequestDto = {
      id: editingId,
      provider_id: editProviderId, // empty string = unbind
      model: editModel,
      tools: undefined,
      context_budget_tokens: undefined,
      timeout_seconds: undefined,
      write_access: undefined,
    };
    updateProfile.mutate(req, {
      onSuccess: () => {
        setEditingId(null);
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "profile.updated",
          message: "Profile updated",
          details: { id: editingId },
        });
      },
      onError: (err) => {
        const msg = (err as Error)?.message ?? String(err);
        setMutationError(msg);
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "profile.update.failed",
          message: "Profile update failed",
          details: { id: editingId, error: msg },
        });
      },
    });
  }, [editingId, editProviderId, editModel, updateProfile]);

  const handleEditCancel = useCallback(() => {
    setEditingId(null);
    setMutationError(null);
  }, []);

  const handleDelete = useCallback(
    (id: string) => {
      setMutationError(null);
      deleteProfile.mutate(id, {
        onError: (err) => {
          const msg = (err as Error)?.message ?? String(err);
          setMutationError(msg);
          reportFrontendEventSafely({
            level: "ERROR",
            event_code: "profile.delete.failed",
            message: "Profile delete failed",
            details: { id, error: msg },
          });
        },
      });
    },
    [deleteProfile],
  );

  if (snapshotQuery.isLoading) {
    return <PageState kind="loading" title="Profiles" message="Loading profiles..." />;
  }
  if (snapshotQuery.isError) {
    return <PageState kind="error" title="Profiles" message="Failed to load profiles" />;
  }

  return (
    <section className="settings-section">
      <h2>Profiles</h2>
      {mutationError && (
        <div className="settings-panel">
          <SettingsRow
            label="Error"
            control={<SettingsValue value={mutationError} tone="danger" />}
          />
        </div>
      )}
      {profiles.length === 0 ? (
        <div className="settings-panel">
          <p>No profiles configured.</p>
        </div>
      ) : (
        profiles.map((profile) => (
          <ProfileRow
            key={profile.id}
            profile={profile}
            providers={providers}
            isEditing={editingId === profile.id}
            editProviderId={editProviderId}
            editModel={editModel}
            onEdit={handleEdit}
            onEditChange={handleEditChange}
            onEditSubmit={handleEditSubmit}
            onEditCancel={handleEditCancel}
            isEditPending={updateProfile.isPending}
            onDelete={handleDelete}
            isDeletePending={deleteProfile.isPending}
          />
        ))
      )}
    </section>
  );
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd apps/gui && pnpm vitest run src/components/ProfilesSection.test.tsx 2>&1 | tail -15`
Expected: All 15 tests PASS.

- [ ] **Step 5: Add `ProfilesSection` to `ProvidersPage`**

In `apps/gui/src/pages/ProvidersPage.tsx`, import and render the section. Add at the top:

```tsx
import { ProfilesSection } from "../components/ProfilesSection";
```

Then in the JSX, after the "Configured providers" section (before the closing `</div>` of `settings-pane`), add:

```tsx
        <ProfilesSection />
```

- [ ] **Step 6: Update `ProvidersPage.test.tsx` mocks**

`ProvidersPage` now renders `ProfilesSection`, which calls `useSettingsSnapshot` and `useProfileMutations`. These must be mocked or the existing provider tests will fail.

First, update the import at **line 22** to add `useSettingsSnapshot` and `useProfileMutations`:

```tsx
import { useProviders, useProviderMutations, useSettingsSnapshot, useProfileMutations } from "../api/useBusytokData";
```

Then update the `vi.mock` factory to include the new hooks:

```tsx
vi.mock("../api/useBusytokData", () => ({
  useProviders: vi.fn(),
  useProviderMutations: vi.fn(),
  useSettingsSnapshot: vi.fn(),
  useProfileMutations: vi.fn(),
}));
```

Then declare the mock handles and provide default return values in `beforeEach`:

```tsx
const mockUseSettingsSnapshot = vi.mocked(useSettingsSnapshot);
const mockUseProfileMutations = vi.mocked(useProfileMutations);

// In beforeEach (after the existing mockUseProviders / mockUseProviderMutations defaults):
mockUseSettingsSnapshot.mockReturnValue({
  data: {
    data: {
      subagent: {
        enabled: true,
        profiles: [],
      },
    },
  },
  isLoading: false,
  isError: false,
  isFetching: false,
} as never);
mockUseProfileMutations.mockReturnValue({
  createProfile: { mutate: vi.fn(), isPending: false },
  updateProfile: { mutate: vi.fn(), isPending: false },
  deleteProfile: { mutate: vi.fn(), isPending: false },
} as never);
```

- [ ] **Step 7: Run all GUI tests**

Run: `cd apps/gui && pnpm vitest run 2>&1 | tail -15`
Expected: All tests PASS.

- [ ] **Step 8: Check coverage**

Run: `cd apps/gui && pnpm vitest run --coverage 2>&1 | grep -E "ProfilesSection|ProvidersPage"`
Expected: `ProfilesSection.tsx` coverage >90%.

- [ ] **Step 9: Run full workspace test suite**

Run: `cargo test --workspace 2>&1 | tail -10`
Expected: All tests PASS.

- [ ] **Step 10: Commit**

```bash
git add apps/gui/src/components/ProfilesSection.tsx apps/gui/src/components/ProfilesSection.test.tsx apps/gui/src/pages/ProvidersPage.tsx apps/gui/src/pages/ProvidersPage.test.tsx
git commit -m "feat(gui): add ProfilesSection to ProvidersPage

Phase 4 Task 6: Profile management section with provider dropdown (enabled
only), cascade-filtered model dropdown, stale-model warning, disabled-
provider warning, read-only advanced fields (tools/budget/timeout), and
delete button (hidden for built-in profiles). Editable: provider_id +
model; immutable: id + is_builtin. Coverage >90%."
```

---

## Acceptance Criteria Checklist (spec §6 Phase 4)

- [ ] Built-in profiles visible in Providers page (Task 6 — ProfilesSection renders them)
- [ ] User can bind profile to provider + model (Task 6 — Edit → dropdown → Save)
- [ ] Disabled provider excluded from new bindings (Task 4 — `profile_update` rejects; Task 6 — dropdown filters)
- [ ] Stale model shows invalid state (Task 6 — `isStaleModel()` warning)
- [ ] Service canonicalizes missing built-in profiles on load (Task 1 — `canonicalize_builtin_profiles()`)
- [ ] `profile.update` supports partial/patch semantics (Task 4 — `Option<T>` fields, `None` = unchanged)

## Excluded from Phase 4 (per spec deferrals)

- Auto-retry of failed tasks (Phase 3 deferral)
- Real SDK memory compaction for `prepare_hibernate`
- Non-OpenAI providers (`anthropic`, `google`)
- Streaming mode
- `pi/patch-small` write-mode profile
- `--no-wait` / `task.*` commands
- `task_queue_max` enforcement
- 3 Phase-5 doctor checks (`bundled_node_arch`, `bundle_manifest_readable`, `pi_runtime_installed`)
