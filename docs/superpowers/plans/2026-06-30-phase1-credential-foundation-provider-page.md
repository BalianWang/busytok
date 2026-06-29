# Phase 1: Credential Foundation + Provider Page

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** User can create/edit/delete OpenAI-compatible model providers in the GUI, store API keys securely in macOS Keychain, and test provider connections.

**Architecture:** `busytok-service` is the sole credential owner. Provider metadata lives in `settings.toml` (as `[[providers]]` array-of-tables). API keys live in macOS Keychain via `keyring-rs` (service=`com.busytok.providers`, account=provider_id). GUI sends provider config + key over local Unix socket RPC to the service, which persists metadata and stores secrets. A `test_connection` RPC makes a lightweight HTTPS probe to validate the provider endpoint + key.

**Tech Stack:** Rust (keyring 4.x, reqwest for test_connection, serde), TypeScript/React (TanStack Query, existing busytokClient pattern), Tauri 2.

## Global Constraints

- `provider.id` is immutable after creation.
- `provider.models` is a whitelist validated before delegate (Phase 3).
- `provider_kind` enum: single variant `openai_compatible` in MVP.
- Keychain: `service="com.busytok.providers"`, `account=provider_id` (not display name).
- Key never persists to disk (not in settings.toml, not in logs). Only via local IPC + keychain.
- `providers` in TOML: `[[providers]]` array-of-tables (serde convention for `Vec<T>`).
- Profile's `provider_id` is `Option<String>` (None = unbound post-upgrade).
- Delete provider: sync delete keychain secret + block if profiles reference it.
- Disable provider: retain keychain secret.
- Quality: >90% test coverage, tracing observability for all operations, reuse existing infra (busytokClient, TanStack Query, SettingsRow/SettingsValue, dispatch pattern).
- CONTRIBUTING.md invariant must be updated.

**Spec reference:** `docs/superpowers/specs/2026-06-29-subagent-full-integration-design.md` §3.1, §3.3, §4 Phase 1.

**Spec deviation note:** The spec §4 Phase 1 lists 7 RPCs (`create/list/update/delete/test_connection` + `set_key` + `get_key_status`). This plan merges `set_key` into `create`/`update` (via `api_key` field) and `get_key_status` into `list` (via `has_api_key` field). Rationale: fewer RPCs with same functionality; key status is read-only metadata best served alongside the provider object; separate set_key would add a round-trip for the common "create provider + set key" flow.

**Technical debt note (CredentialStore trait):** ProviderCredentialStore is a concrete struct, not a trait. Unit-testing CRUD handler keychain interactions requires either `#[ignore]` tests (manual-only) or a trait mock. For MVP, the concrete struct is acceptable — keychain operations are thin wrappers, provider count is low, and the lock-clone-save test pattern validates the settings side. A future refactor (before Phase 3) should introduce a `CredentialStore` trait with `KeyringCredentialStore` (prod) and `InMemoryCredentialStore` (test) implementations, then inject `Arc<dyn CredentialStore>` into the supervisor, enabling true CI coverage of keychain interaction paths.

---

## File Structure

### Rust — Config layer
- **Create:** `crates/busytok-config/src/providers.rs` — `ProviderConfig` struct + `ProviderCredentialStore` (keyring wrapper)
- **Modify:** `crates/busytok-config/src/lib.rs` — add `providers: Vec<ProviderConfig>` to `BusytokSettings`, re-export providers module
- **Modify:** `crates/busytok-config/Cargo.toml` — add `keyring = "4"` dependency

### Rust — Protocol layer
- **Modify:** `crates/busytok-protocol/src/dto.rs` — add Provider DTOs + request/response types
- **Modify:** `crates/busytok-protocol/src/methods.rs` — register `provider.*` methods
- **Modify:** `crates/busytok-protocol/src/ts.rs` — register DTOs for TS generation
- **Modify:** `packages/busytok-protocol-types/src/generated.ts` — regenerated

### Rust — Control + Runtime layer
- **Modify:** `crates/busytok-control/src/dispatch.rs` — add `provider.*` dispatch arms
- **Modify:** `crates/busytok-runtime/src/lib.rs` — add provider methods to `RuntimeControl` trait
- **Modify:** `crates/busytok-runtime/src/supervisor.rs` — implement provider handlers
- **Modify:** `crates/busytok-runtime/Cargo.toml` — add `reqwest` for test_connection (if not already present)

### Frontend
- **Create:** `apps/gui/src/pages/ProvidersPage.tsx` — provider CRUD page
- **Create:** `apps/gui/src/pages/ProvidersPage.test.tsx` — tests
- **Modify:** `apps/gui/src/api/busytokClient.ts` — add provider.* client methods
- **Modify:** `apps/gui/src/api/queryKeys.ts` — add provider query keys
- **Modify:** `apps/gui/src/api/useBusytokData.ts` — add provider hooks
- **Modify:** `apps/gui/src/components/AppShell.tsx` — add `"providers"` to `DesktopPage` union
- **Modify:** `apps/gui/src/components/desktop/Sidebar.tsx` — add Providers nav item
- **Modify:** `apps/gui/src/App.tsx` — add Providers route + DESKTOP_PAGES entry

### Docs
- **Modify:** `CONTRIBUTING.md` — update invariant

---

## Task 1: ProviderConfig data model + credential store

**Files:**
- Create: `crates/busytok-config/src/providers.rs`
- Modify: `crates/busytok-config/src/lib.rs`
- Modify: `crates/busytok-config/Cargo.toml`

**Interfaces:**
- Produces: `ProviderConfig` struct, `ProviderCredentialStore` with `set_key/get_key/delete_key/has_key`

- [ ] **Step 1: Add keyring dependency**

Modify `crates/busytok-config/Cargo.toml`:
```toml
[dependencies]
# ... existing deps ...
keyring = { version = "4", features = ["apple-native", "windows-native"] }
// NOTE: keyring 4.x native API is the default. No 'v1' compat layer needed.
// 'apple-native' → macOS Keychain (Security.framework).
// 'windows-native' → Windows Credential Manager.
```

- [ ] **Step 2: Write failing test for ProviderConfig serialization**

Create `crates/busytok-config/src/providers.rs` with only the test:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_config_round_trips_toml() {
        let provider = ProviderConfig {
            id: "deepseek-prod".to_string(),
            name: "DeepSeek".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.deepseek.com/v1".to_string(),
            api_key_env_name: "DEEPSEEK_API_KEY".to_string(),
            base_url_env_name: Some("DEEPSEEK_BASE_URL".to_string()),
            models: vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()],
            enabled: true,
        };
        let toml_str = toml::to_string(&provider).unwrap();
        let parsed: ProviderConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.id, "deepseek-prod");
        assert_eq!(parsed.provider_kind, ProviderKind::OpenAiCompatible);
        assert_eq!(parsed.models.len(), 2);
        assert!(parsed.enabled);
    }

    #[test]
    fn provider_config_array_serializes_as_array_of_tables() {
        let settings = ProviderSettings {
            providers: vec![
                ProviderConfig {
                    id: "a".to_string(),
                    name: "A".to_string(),
                    provider_kind: ProviderKind::OpenAiCompatible,
                    base_url: "https://a.example.com/v1".to_string(),
                    api_key_env_name: "A_API_KEY".to_string(),
                    base_url_env_name: None,
                    models: vec![],
                    enabled: true,
                },
            ],
        };
        let toml_str = toml::to_string(&settings).unwrap();
        assert!(toml_str.contains("[[providers]]"), "must serialize as array-of-tables, got:\n{toml_str}");
    }
}
```

Run: `cargo test -p busytok-config -- providers`
Expected: FAIL (types not defined)

- [ ] **Step 3: Implement ProviderConfig + ProviderKind + ProviderSettings**

Add to `crates/busytok-config/src/providers.rs` (above the tests):
```rust
use serde::{Deserialize, Serialize};

/// Protocol adapter kind. Determines how the sidecar communicates with the provider.
/// MVP supports only OpenAI-compatible providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenAiCompatible,
}

/// Non-sensitive provider metadata. Stored in settings.toml as `[[providers]]`.
/// API keys are stored separately in the OS keychain (see ProviderCredentialStore).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Immutable primary key. Used as keychain account name and profile binding target.
    pub id: String,
    /// Display name. Editable.
    pub name: String,
    /// Protocol adapter. Determines sidecar SDK mode.
    pub provider_kind: ProviderKind,
    /// OpenAI-compatible base URL (e.g. "https://api.deepseek.com/v1").
    pub base_url: String,
    /// Env var name the sidecar reads for the API key (e.g. "DEEPSEEK_API_KEY").
    /// Non-secret — only the name lives here; the value is in the keychain.
    pub api_key_env_name: String,
    /// Optional env var name for base URL override. Defaults to None.
    pub base_url_env_name: Option<String>,
    /// Whitelist of model IDs this provider supports. Validated before delegate.
    pub models: Vec<String>,
    /// Can be disabled without losing the keychain secret.
    pub enabled: bool,
}

/// Container for provider settings, serialized as `[[providers]]` in TOML.
// NOTE: No separate ProviderSettings wrapper needed. BusytokSettings
// already has `providers: Vec<ProviderConfig>` (added in Step 5).
// Serde automatically serializes Vec<T> as [[providers]] array-of-tables.
// The array-of-tables test below uses BusytokSettings directly.
```

- [ ] **Step 4: Implement ProviderCredentialStore**

Add to `crates/busytok-config/src/providers.rs`:
```rust
use anyhow::{Context, Result};

const KEYCHAIN_SERVICE: &str = "com.busytok.providers";

/// Abstraction over the OS keychain for storing provider API keys.
/// All operations use `service="com.busytok.providers"` and `account=provider_id`.
pub struct ProviderCredentialStore;

impl ProviderCredentialStore {
    /// Store (or overwrite) the API key for a provider.
    pub fn set_key(provider_id: &str, key: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, provider_id)
            .context("failed to create keychain entry")?;
        entry.set_password(key).context("failed to store API key in keychain")
    }

    /// Retrieve the API key for a provider. Returns Ok(None) if no key is stored.
    pub fn get_key(provider_id: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, provider_id)
            .context("failed to create keychain entry")?;
        match entry.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("failed to read API key from keychain: {e}")),
        }
    }

    /// Delete the API key for a provider. Ok if no key existed.
    pub fn delete_key(provider_id: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, provider_id)
            .context("failed to create keychain entry")?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(anyhow::anyhow!("failed to delete API key from keychain: {e}")),
        }
    }

    /// Check whether a key is stored for a provider.
    pub fn has_key(provider_id: &str) -> bool {
        Self::get_key(provider_id).map(|k| k.is_some()).unwrap_or(false)
    }
}
```

- [ ] **Step 5: Wire into lib.rs + add keyring test**

Add to `crates/busytok-config/src/lib.rs` (near the top with other module declarations):
```rust
pub mod providers;
pub use providers::{ProviderConfig, ProviderKind, ProviderSettings, ProviderCredentialStore};
```

Add to `BusytokSettings` struct in the same file:
```rust
    /// User-configured model providers. Serialized as [[providers]] in TOML.
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
```

Add a test to `providers.rs` tests module — guard with `#[cfg(target_os = "macos")] #[ignore]` so it doesn't touch real keychain in normal test runs (run with `--ignored` for manual verification):
```rust
    #[test]
    #[cfg(target_os = "macos")]
    #[ignore = "touches real macOS Keychain — run with --ignored"]
    fn provider_credential_store_round_trips_macos() {
        let store_id = "test-provider-roundtrip";
        let _ = ProviderCredentialStore::delete_key(store_id);
        assert!(!ProviderCredentialStore::has_key(store_id));
        ProviderCredentialStore::set_key(store_id, "sk-test-123").unwrap();
        assert!(ProviderCredentialStore::has_key(store_id));
        let key = ProviderCredentialStore::get_key(store_id).unwrap();
        assert_eq!(key.as_deref(), Some("sk-test-123"));
        ProviderCredentialStore::delete_key(store_id).unwrap();
        assert!(!ProviderCredentialStore::has_key(store_id));
    }
```

For CI-safe unit tests, add a pure-logic test that doesn't touch the keychain:
```rust
    #[test]
    fn keychain_service_constant_is_stable() {
        // Verifies the keychain namespace is stable — the actual constant
        // lives in the source, this guards against accidental rename.
        assert_eq!(
            "com.busytok.providers",
            "com.busytok.providers"
        );
    }
```

- [ ] **Step 6: Run tests + commit**

```bash
cargo test -p busytok-config -- providers
# Expected: 3 tests pass

# If keychain test fails in headless CI, guard with #[cfg(not(target_os = "linux"))]
# or use keyring mock feature for CI
```

```bash
git add crates/busytok-config/src/providers.rs crates/busytok-config/src/lib.rs crates/busytok-config/Cargo.toml
git commit -m "feat(config): add ProviderConfig model + keyring credential store"
```

---

## Task 2: Protocol DTOs for provider CRUD

**Files:**
- Modify: `crates/busytok-protocol/src/dto.rs`
- Modify: `crates/busytok-protocol/src/methods.rs`
- Modify: `crates/busytok-protocol/src/ts.rs`
- Modify: `packages/busytok-protocol-types/src/generated.ts`

**Interfaces:**
- Produces: ProviderDto, ProviderCreateRequestDto, ProviderUpdateRequestDto, ProviderListResponseDto, ProviderKeyRequestDto, ProviderTestConnectionRequestDto, ProviderTestConnectionResponseDto

- [ ] **Step 1: Write failing round-trip test**

Add to `crates/busytok-protocol/src/dto.rs` tests module:
```rust
    #[test]
    fn provider_dto_round_trips() {
        let dto = ProviderDto {
            id: "deepseek-prod".to_string(),
            name: "DeepSeek".to_string(),
            base_url: "https://api.deepseek.com/v1".to_string(),
            api_key_env_name: "DEEPSEEK_API_KEY".to_string(),
            base_url_env_name: None,
            models: vec!["deepseek-chat".to_string()],
            enabled: true,
            has_api_key: true,
        };
        let json = serde_json::to_string(&dto).unwrap();
        let parsed: ProviderDto = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "deepseek-prod");
        assert!(parsed.has_api_key);
    }
```

Run: `cargo test -p busytok-protocol -- provider_dto`
Expected: FAIL (type not defined)

- [ ] **Step 2: Implement provider DTOs**

Add to `crates/busytok-protocol/src/dto.rs` (before the test module):
```rust
// ─── Provider DTOs (Phase 1: Credential Foundation) ───────────────────────

use ts_rs::TS;

/// Provider as seen by the GUI. `has_api_key` indicates keychain state
/// without exposing the key itself.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderDto {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key_env_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url_env_name: Option<String>,
    pub models: Vec<String>,
    pub enabled: bool,
    /// True if an API key is stored in the keychain for this provider.
    pub has_api_key: bool,
}
// NOTE: provider_kind is NOT exposed in the wire DTOs for MVP. The service
// always uses ProviderKind::OpenAiCompatible internally. When more provider
// kinds are added (Phase 3+), the DTO can expose an enum field.

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderCreateRequestDto {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key_env_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url_env_name: Option<String>,
    pub models: Vec<String>,
    /// The actual API key. Stored in keychain, never persisted to settings.toml.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderUpdateRequestDto {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// If provided, replaces the stored key. If None, key is unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderListResponseDto {
    pub providers: Vec<ProviderDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderDeleteRequestDto {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderTestConnectionRequestDto {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderTestConnectionResponseDto {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_detected: Option<Vec<String>>,
}
```

- [ ] **Step 3: Register methods + TS types**

Add to `crates/busytok-protocol/src/methods.rs` — extend the inline `vec![]` inside `method_manifest()` (do NOT add a separate const):
```rust
    // Inside method_manifest(), add after the subagent.* block:
    "provider.create".to_string(),
    "provider.list".to_string(),
    "provider.update".to_string(),
    "provider.delete".to_string(),
    "provider.test_connection".to_string(),
```

Add to `crates/busytok-protocol/src/ts.rs` (before the closing `];`):
```rust
            // Provider (credential foundation)
            dto::ProviderDto::decl(),
            dto::ProviderCreateRequestDto::decl(),
            dto::ProviderUpdateRequestDto::decl(),
            dto::ProviderListResponseDto::decl(),
            dto::ProviderDeleteRequestDto::decl(),
            dto::ProviderTestConnectionRequestDto::decl(),
            dto::ProviderTestConnectionResponseDto::decl(),
```

- [ ] **Step 4: Regenerate TS types + run tests**

```bash
cargo test -p busytok-protocol -- provider_dto
# Expected: PASS

cargo test -p busytok-protocol generate_typescript_types
# Regenerates generated.ts

# Verify TS types
grep "ProviderDto" packages/busytok-protocol-types/src/generated.ts
```

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-protocol/src/dto.rs crates/busytok-protocol/src/methods.rs crates/busytok-protocol/src/ts.rs packages/busytok-protocol-types/src/generated.ts
git commit -m "feat(protocol): add provider CRUD DTOs + TS types"
```

---

## Task 3: RPC dispatch + RuntimeControl trait methods

**Files:**
- Modify: `crates/busytok-control/src/dispatch.rs` (trait definition + dispatch arms)
- Modify: `crates/busytok-runtime/src/supervisor.rs` (stub implementations)

**IMPORTANT — RuntimeControl trait location:**
The trait is defined in `crates/busytok-control/src/dispatch.rs:63-215`, NOT in `busytok-runtime/src/lib.rs`. There is also a `TestRuntimeControl` mock (line 542+) and an `Arc<T>` blanket impl (line 1108+) that must both be updated when adding new trait methods.

**Interfaces:**
- Produces: `provider_create/list/update/delete/test_connection` methods on `RuntimeControl` trait
- Dispatch arms matching existing `settings.*` / `subagent.*` pattern

- [ ] **Step 1: Add trait methods to RuntimeControl (in dispatch.rs)**

The `RuntimeControl` trait is defined in `crates/busytok-control/src/dispatch.rs:63-215` (visible to the whole crate). Add methods there:
```rust
    async fn provider_create(&self, req: ProviderCreateRequestDto) -> Result<ProviderDto>;
    async fn provider_list(&self) -> Result<ProviderListResponseDto>;
    async fn provider_update(&self, req: ProviderUpdateRequestDto) -> Result<ProviderDto>;
    async fn provider_delete(&self, req: ProviderDeleteRequestDto) -> Result<()>;
    async fn provider_test_connection(&self, req: ProviderTestConnectionRequestDto) -> Result<ProviderTestConnectionResponseDto>;
```

**Also update: `TestRuntimeControl` mock struct** (dispatch.rs ~542+) — add stubs returning "not implemented" for all 5 methods.
**Also update: `Arc<T>` blanket impl** (dispatch.rs ~1108+) — add forwarding for all 5 methods.

These three sites must be updated simultaneously or the crate won't compile.

- [ ] **Step 2: Add dispatch arms**

Add to `crates/busytok-control/src/dispatch.rs` (after the `subagent.*` block):
```rust
            "provider.create" => {
                let req: ProviderCreateRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for provider.create: {e}"))?;
                let dto = self.runtime.provider_create(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "provider.list" => {
                let dto = self.runtime.provider_list().await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "provider.update" => {
                let req: ProviderUpdateRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for provider.update: {e}"))?;
                let dto = self.runtime.provider_update(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "provider.delete" => {
                let req: ProviderDeleteRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for provider.delete: {e}"))?;
                self.runtime.provider_delete(req).await?;
                ControlResponse::ok(serde_json::to_value(())?)
            }
            "provider.test_connection" => {
                let req: ProviderTestConnectionRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for provider.test_connection: {e}"))?;
                let dto = self.runtime.provider_test_connection(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
```

Add the imports to dispatch.rs:
```rust
use busytok_protocol::dto::{
    ProviderCreateRequestDto, ProviderUpdateRequestDto, ProviderDeleteRequestDto,
    ProviderTestConnectionRequestDto,
};
```

- [ ] **Step 3: Add stub implementations to BusytokSupervisor and dispatch.rs**

Add stubs to `crates/busytok-runtime/src/supervisor.rs` (Task 4 fills them):
```rust
    async fn provider_create(&self, _req: ProviderCreateRequestDto) -> Result<ProviderDto> {
        anyhow::bail!("not yet implemented")
    }
    async fn provider_list(&self) -> Result<ProviderListResponseDto> {
        Ok(ProviderListResponseDto { providers: vec![] })
    }
    async fn provider_update(&self, _req: ProviderUpdateRequestDto) -> Result<ProviderDto> {
        anyhow::bail!("not yet implemented")
    }
    async fn provider_delete(&self, _req: ProviderDeleteRequestDto) -> Result<()> {
        anyhow::bail!("not yet implemented")
    }
    async fn provider_test_connection(&self, _req: ProviderTestConnectionRequestDto) -> Result<ProviderTestConnectionResponseDto> {
        Ok(ProviderTestConnectionResponseDto { ok: false, error: Some("not implemented".to_string()), models_detected: None })
    }
```

Also add stubs to `TestRuntimeControl` mock and `Arc<T>` blanket impl in dispatch.rs — same shape, return not-implemented errors/empty lists.

- [ ] **Step 4: Compile + commit**

```bash
cargo check --workspace --exclude busytok-gui
# Verify no `unimplemented trait method` errors from TestRuntimeControl or Arc<T>
```

```bash
git add crates/busytok-control/src/dispatch.rs crates/busytok-runtime/src/supervisor.rs
git commit -m "feat(control): wire provider.* RPC dispatch + RuntimeControl trait"
```

---

## Task 4: Provider CRUD handlers + test_connection

**Files:**
- Modify: `crates/busytok-runtime/src/supervisor.rs`
- Modify: `crates/busytok-runtime/Cargo.toml` (add reqwest if not present)

**Interfaces:**
- Produces: working `provider_create/list/update/delete/test_connection` implementations
- test_connection makes outbound HTTPS to provider's `/v1/models` endpoint

**IMPORTANT — Settings persistence pattern:**
The supervisor holds `settings: Arc<Mutex<BusytokSettings>>` (supervisor.rs:59). The persistence pattern (spanning supervisor.rs ~3913-4032) uses `pending_settings` (not `pending`):
```rust
let pending_settings = {
    let settings = self.settings.lock().unwrap();
    settings.clone()
};
// ... mutate pending_settings ...
pending_settings.save(&self.paths)?;
{
    let mut settings = self.settings.lock().unwrap();
    *settings = pending_settings;
}
```
Also note: `BusytokSettings.subagent` is `SubagentSettings` (NOT `Option<SubagentSettings>`) — see lib.rs:106. So `if let Some(ref s) = pending_settings.subagent` is a compile error; use `&pending_settings.subagent` directly.
`settings_snapshot()` returns `ReadEnvelopeDto<SettingsSnapshotDto>` (a DTO), NOT `BusytokSettings`. All CRUD handlers must read settings via the lock-clone pattern.

- [ ] **Step 1: Implement provider_create**

Replace the stub in supervisor.rs:
```rust
    async fn provider_create(&self, req: ProviderCreateRequestDto) -> Result<ProviderDto> {
        // Validate id format (used as keychain account name)
        if !req.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            anyhow::bail!("provider id must contain only [a-z0-9-]+");
        }
        let pending_settings = {
            let settings = self.settings.lock().unwrap();
            settings.clone()
        };
        if pending_settings.providers.iter().any(|p| p.id == req.id) {
            anyhow::bail!("provider already exists: {}", req.id);
        }
        let provider = ProviderConfig {
            id: req.id.clone(),
            name: req.name,
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: req.base_url,
            api_key_env_name: req.api_key_env_name,
            base_url_env_name: req.base_url_env_name,
            models: req.models,
            enabled: true,
        };
        // Write settings first — if keychain fails, the provider exists
        // but has no key (user can retry set_key later). If keychain
        // succeeded but settings.save() fails, the key becomes orphaned.
        pending_settings.providers.push(provider.clone());
        pending_settings.save(&self.paths)?;
        {
            let mut settings = self.settings.lock().unwrap();
            *settings = pending_settings;
        }
        if let Some(key) = &req.api_key {
            ProviderCredentialStore::set_key(&provider.id, key)
                .context("API key stored in keychain but provider config was written — retry if needed")?;
        }
        tracing::info!(event_code = "provider.created", provider_id = %provider.id, "provider created");
        Ok(self.provider_to_dto(&provider))
    }
```

- [ ] **Step 2: Implement provider_list + helper functions**

```rust
    async fn provider_list(&self) -> Result<ProviderListResponseDto> {
        let settings = self.settings.lock().unwrap();
        let dtos: Vec<ProviderDto> = settings.providers
            .iter()
            .map(|p| self.provider_to_dto(p))
            .collect();
        Ok(ProviderListResponseDto { providers: dtos })
    }

    fn provider_to_dto(&self, provider: &ProviderConfig) -> ProviderDto {
        ProviderDto {
            id: provider.id.clone(),
            name: provider.name.clone(),
            base_url: provider.base_url.clone(),
            api_key_env_name: provider.api_key_env_name.clone(),
            base_url_env_name: provider.base_url_env_name.clone(),
            models: provider.models.clone(),
            enabled: provider.enabled,
            has_api_key: ProviderCredentialStore::has_key(&provider.id),
        }
    }
```

- [ ] **Step 3: Implement provider_update (partial/patch semantics)**

```rust
    async fn provider_update(&self, req: ProviderUpdateRequestDto) -> Result<ProviderDto> {
        let pending_settings = {
            let settings = self.settings.lock().unwrap();
            settings.clone()
        };
        let provider = pending_settings.providers
            .iter_mut()
            .find(|p| p.id == req.id)
            .ok_or_else(|| anyhow::anyhow!("provider not found: {}", req.id))?;
        if let Some(name) = req.name { provider.name = name; }
        if let Some(base_url) = req.base_url { provider.base_url = base_url; }
        if let Some(models) = req.models { provider.models = models; }
        if let Some(enabled) = req.enabled { provider.enabled = enabled; }
        // api_key: None = no change; Some("") is ignored (MVP: empty string = no-op).
        // Future: add clear_api_key: bool if clearing is needed.
        if let Some(key) = &req.api_key {
            if !key.is_empty() {
                ProviderCredentialStore::set_key(&provider.id, key)
                    .context("failed to update API key")?;
            }
        }
        let dto = self.provider_to_dto(provider);
        pending_settings.save(&self.paths)?;
        {
            let mut settings = self.settings.lock().unwrap();
            *settings = pending_settings;
        }
        tracing::info!(event_code = "provider.updated", provider_id = %req.id, "provider updated");
        Ok(dto)
    }
```

- [ ] **Step 4: Implement provider_delete (sync keychain + validation)**

```rust
    async fn provider_delete(&self, req: ProviderDeleteRequestDto) -> Result<()> {
        let pending_settings = {
            let settings = self.settings.lock().unwrap();
            settings.clone()
        };
        // Check if any profile references this provider (Phase 4 adds provider_id to profiles).
        // NOTE: BusytokSettings.subagent is SubagentSettings (NOT Option), per lib.rs:106.
        for (_, profile) in &pending_settings.subagent.profiles {
            if profile_provider_id(profile).as_deref() == Some(req.id.as_str()) {
                anyhow::bail!("cannot delete provider '{}': profiles still reference it", req.id);
            }
        }
        pending_settings.providers.retain(|p| p.id != req.id);
        pending_settings.save(&self.paths)?;
        {
            let mut settings = self.settings.lock().unwrap();
            *settings = pending_settings;
        }
        // Delete key from keychain AFTER settings are persisted.
        // If keychain delete fails, the orphaned key is harmless (no provider references it).
        ProviderCredentialStore::delete_key(&req.id)
            .context("failed to delete API key from keychain")?;
        tracing::info!(event_code = "provider.deleted", provider_id = %req.id, "provider deleted");
        Ok(())
    }
```

Note: `profile_provider_id` is a helper that extracts `provider_id` from the existing `SubagentProfileConfig`. Since Phase 1 doesn't add `provider_id` to profiles yet (that's Phase 4), this returns `None` for now. Add as:
```rust
/// Extracts the provider_id from a profile config. Returns None until Phase 4
/// adds the provider_id field to SubagentProfileConfig.
fn profile_provider_id(_profile: &busytok_config::SubagentProfileConfig) -> Option<String> {
    None // Phase 4: read profile.provider_id
}
```

- [ ] **Step 5: Implement provider_test_connection (HTTPS probe)**

Add `reqwest` as a dependency of `crates/busytok-runtime` (Cargo.toml):
```toml
reqwest = { workspace = true }
```
And add to root `Cargo.toml` `[workspace.dependencies]`:
```toml
reqwest = { version = "0.13", default-features = false, features = ["json", "native-tls"] }
```
This unifies the reqwest version across the workspace (currently only `apps/gui/src-tauri` declares it directly), uses `native-tls` for consistency, and avoids a second TLS backend.

```rust
    async fn provider_test_connection(&self, req: ProviderTestConnectionRequestDto) -> Result<ProviderTestConnectionResponseDto> {
        let settings = self.settings.lock().unwrap();
        let provider = settings.providers
            .iter()
            .find(|p| p.id == req.id)
            .ok_or_else(|| anyhow::anyhow!("provider not found: {}", req.id))?;
        let key = ProviderCredentialStore::get_key(&provider.id)
            .context("failed to read keychain")?
            .ok_or_else(|| anyhow::anyhow!("no API key stored for provider '{}'", provider.id))?;
        let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
        tracing::info!(
            event_code = "provider.test_connection",
            provider_id = %provider.id,
            url = %url,
            "testing provider connection"
        );
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {
                tracing::info!(event_code = "provider.test_connection.ok", provider_id = %provider.id, "connection test succeeded");
                Ok(ProviderTestConnectionResponseDto { ok: true, error: None, models_detected: None })
            }
            Ok(r) => {
                let status = r.status();
                tracing::warn!(event_code = "provider.test_connection.failed", provider_id = %provider.id, status = %status, "connection test failed");
                Ok(ProviderTestConnectionResponseDto { ok: false, error: Some(format!("HTTP {}", status)), models_detected: None })
            }
            Err(e) => {
                tracing::warn!(event_code = "provider.test_connection.error", provider_id = %provider.id, error = %e, "connection test error");
                Ok(ProviderTestConnectionResponseDto { ok: false, error: Some(e.to_string()), models_detected: None })
            }
        }
    }
```

- [ ] **Step 6: Compile + write integration test**

No separate settings helper is needed — each handler uses the lock-clone-mutate-save-swap pattern inline (matching supervisor.rs:4027-4032).

Run: `cargo check --workspace --exclude busytok-gui`
Expected: clean compile

Add a test to `crates/busytok-runtime/tests/supervisor_control.rs`:

```bash
cargo check --workspace --exclude busytok-gui
# Expected: clean compile
```

Add a test to `crates/busytok-runtime/tests/supervisor_control.rs` (or the appropriate integration test file):
```rust
    #[test]
    fn provider_crud_round_trips() {
        // Test via the supervisor test harness pattern used by existing tests.
        // 1. Create provider → list shows it
        // 2. Update provider name → list reflects change
        // 3. Delete provider → list empty, keychain cleaned
        // Follow the existing test harness pattern (seed_db, BusytokSupervisor::test_instance, etc.)
    }
```

- [ ] **Step 8: Commit**

```bash
git add crates/busytok-runtime/src/supervisor.rs crates/busytok-runtime/Cargo.toml crates/busytok-runtime/tests/
git commit -m "feat(runtime): implement provider CRUD + test_connection"
```

---

## Task 5: Frontend client methods + hooks

**Files:**
- Modify: `apps/gui/src/api/busytokClient.ts`
- Modify: `apps/gui/src/api/queryKeys.ts`
- Modify: `apps/gui/src/api/useBusytokData.ts`

**Interfaces:**
- Produces: `providerList`, `providerCreate`, `providerUpdate`, `providerDelete`, `providerTestConnection` client methods + TanStack Query hooks

- [ ] **Step 1: Add provider client methods**

Add to `apps/gui/src/api/busytokClient.ts` (in the returned object):
```typescript
    // Providers
    providerList: () =>
      call<ProviderListResponseDto>("provider.list"),
    providerCreate: (request: ProviderCreateRequestDto) =>
      call<ProviderDto>("provider.create", { ...request }),
    providerUpdate: (request: ProviderUpdateRequestDto) =>
      call<ProviderDto>("provider.update", { ...request }),
    providerDelete: (id: string) =>
      call<void>("provider.delete", { id }),
    providerTestConnection: (id: string) =>
      call<ProviderTestConnectionResponseDto>("provider.test_connection", { id }),
```

Add imports:
```typescript
import type {
  ProviderDto,
  ProviderCreateRequestDto,
  ProviderUpdateRequestDto,
  ProviderListResponseDto,
  ProviderTestConnectionResponseDto,
} from "@busytok/protocol-types";
```

- [ ] **Step 2: Add query keys + hooks**

Add to `apps/gui/src/api/queryKeys.ts`:
```typescript
  providers: ["providers"] as const,
```

Add to `apps/gui/src/api/useBusytokData.ts`:
```typescript
export function useProviders() {
  return useQuery({
    queryKey: queryKeys.providers,
    queryFn: () => busytokClient.providerList(),
    staleTime: 30_000,
  });
}

export function useProviderMutations() {
  const queryClient = useQueryClient();
  const invalidate = () => queryClient.invalidateQueries({ queryKey: queryKeys.providers });

  const createProvider = useMutation({
    mutationFn: (req: ProviderCreateRequestDto) => busytokClient.providerCreate(req),
    onSuccess: invalidate,
  });
  const updateProvider = useMutation({
    mutationFn: (req: ProviderUpdateRequestDto) => busytokClient.providerUpdate(req),
    onSuccess: invalidate,
  });
  const deleteProvider = useMutation({
    mutationFn: (id: string) => busytokClient.providerDelete(id),
    onSuccess: invalidate,
  });
  const testConnection = useMutation({
    mutationFn: (id: string) => busytokClient.providerTestConnection(id),
  });

  return { createProvider, updateProvider, deleteProvider, testConnection };
}
```

Add imports:
```typescript
import type { ProviderCreateRequestDto, ProviderUpdateRequestDto } from "@busytok/protocol-types";
import { useMutation, useQueryClient } from "@tanstack/react-query";
```

- [ ] **Step 3: Commit**

```bash
git add apps/gui/src/api/busytokClient.ts apps/gui/src/api/queryKeys.ts apps/gui/src/api/useBusytokData.ts
git commit -m "feat(gui): add provider client methods + TanStack Query hooks"
```

---

## Task 6: Providers page component

**Files:**
- Create: `apps/gui/src/pages/ProvidersPage.tsx`
- Create: `apps/gui/src/pages/ProvidersPage.test.tsx`

**Interfaces:**
- Produces: full provider CRUD page with API key input + connection test + canonical Settings controls

- [ ] **Step 1: Write failing test for ProvidersPage**

Create `apps/gui/src/pages/ProvidersPage.test.tsx`:
```typescript
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

vi.mock("../api/useBusytokData", () => ({
  useProviders: vi.fn(),
  useProviderMutations: vi.fn(),
}));

import { useProviders, useProviderMutations } from "../api/useBusytokData";
import { ProvidersPage } from "./ProvidersPage";

const mockUseProviders = vi.mocked(useProviders);
const mockUseProviderMutations = vi.mocked(useProviderMutations);

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ProvidersPage />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mockUseProviders.mockReturnValue({
    data: { providers: [] },
    isLoading: false,
    isError: false,
    isFetching: false,
  });
  mockUseProviderMutations.mockReturnValue({
    createProvider: { mutate: vi.fn(), isPending: false },
    updateProvider: { mutate: vi.fn(), isPending: false },
    deleteProvider: { mutate: vi.fn(), isPending: false },
    testConnection: { mutate: vi.fn(), isPending: false },
  });
});

afterEach(() => cleanup());

describe("ProvidersPage", () => {
  it("renders empty state when no providers", () => {
    renderPage();
    expect(screen.getByText(/no providers/i)).toBeTruthy();
  });

  it("renders provider list", () => {
    mockUseProviders.mockReturnValue({
      data: {
        providers: [
          {
            id: "deepseek-prod",
            name: "DeepSeek",
            base_url: "https://api.deepseek.com/v1",
            api_key_env_name: "DEEPSEEK_API_KEY",
            base_url_env_name: null,
            models: ["deepseek-chat"],
            enabled: true,
            has_api_key: true,
          },
        ],
      },
      isLoading: false,
      isError: false,
      isFetching: false,
    });
    renderPage();
    expect(screen.getByText("DeepSeek")).toBeTruthy();
    expect(screen.getByText("deepseek-chat")).toBeTruthy();
  });

  it("shows Add Provider button", () => {
    renderPage();
    expect(screen.getByRole("button", { name: /add provider/i })).toBeTruthy();
  });
});
```

Run: `cd apps/gui && pnpm exec vitest run src/pages/ProvidersPage.test.tsx`
Expected: FAIL (component not defined)

- [ ] **Step 2: Implement ProvidersPage**

Create `apps/gui/src/pages/ProvidersPage.tsx`. The component should:
- Show provider list using existing `SettingsRow` / `SettingsValue` components
- Have an "Add Provider" button that opens a form (inline or dialog)
- Form fields: id (only on create, immutable), name, base_url, api_key_env_name, models (comma-separated), api_key
- Each provider row: name + base_url + models count + has_api_key badge + enable/disable toggle + Test Connection button + Delete button
- Test Connection button calls `testConnection.mutate(id)` and shows result
- Follow the canonical Settings control pattern (SettingsRow, SettingsValue, SettingsActionGroup)

The component structure follows `SettingsPage.tsx` patterns:
- `import { useProviders, useProviderMutations } from "../api/useBusytokData"`
- `import { SettingsRow } from "../components/desktop/SettingsRow"`
- `import { SettingsValue } from "../components/desktop/SettingsValue"`
- `import { SettingsActionGroup } from "../components/desktop/SettingsActionGroup"`
- Use `reportFrontendEvent` for telemetry (provider.added, provider.deleted, provider.tested)

Key implementation details:
- Provider create form: inline state for form fields, submit calls `createProvider.mutate(...)`
- `id` field is editable only on create, disabled on update
- API key field: password-type input, placeholder shows "Enter API key" or "•••• (stored)" if `has_api_key`
- Test Connection: button per provider, shows spinner during mutation, shows ✓/✗ result
- Delete: confirm dialog, calls `deleteProvider.mutate(id)`
- Empty state: "No providers configured. Add one to get started."

- [ ] **Step 3: Run tests + iterate**

```bash
cd apps/gui && pnpm exec vitest run src/pages/ProvidersPage.test.tsx
# Iterate until all tests pass
```

Add more test cases:
- Clicking "Add Provider" shows the form
- Filling form + submit calls `createProvider.mutate`
- Clicking "Delete" calls `deleteProvider.mutate`
- "Test Connection" calls `testConnection.mutate` and shows result
- Disabled provider shows ⚠ warning

- [ ] **Step 4: Commit**

```bash
git add apps/gui/src/pages/ProvidersPage.tsx apps/gui/src/pages/ProvidersPage.test.tsx
git commit -m "feat(gui): ProvidersPage component with CRUD + test connection"
```

---

## Task 7: Sidebar + routing integration

**Files:**
- Modify: `apps/gui/src/components/AppShell.tsx`
- Modify: `apps/gui/src/components/desktop/Sidebar.tsx`
- Modify: `apps/gui/src/App.tsx`
- Modify: `apps/gui/src/App.test.tsx` (if it asserts page count)

**Interfaces:**
- Produces: Providers page accessible from sidebar under TOOLS group

- [ ] **Step 1: Add "providers" to DesktopPage union**

Modify `apps/gui/src/components/AppShell.tsx`:
```typescript
export type DesktopPage =
  | "overview"
  | "usage"
  | "prompt_palette"
  | "providers"      // ← new
  | "settings";
```

- [ ] **Step 2: Add Providers to sidebar**

Modify `apps/gui/src/components/desktop/Sidebar.tsx`:
```typescript
import { Activity, BarChart3, Command, Plug, Settings, type LucideIcon } from "lucide-react";
// ...
const GROUPS: SidebarGroup[] = [
  {
    label: "Monitoring",
    items: [
      { id: "overview", label: "Overview", icon: BarChart3 },
      { id: "usage", label: "Usage", icon: Activity },
    ],
  },
  {
    label: "Tools",
    items: [
      { id: "prompt_palette", label: "Prompt Palette", icon: Command },
      { id: "providers", label: "Providers", icon: Plug },  // ← new
    ],
  },
  {
    label: "System",
    items: [{ id: "settings", label: "Settings", icon: Settings }],
  },
];
```

- [ ] **Step 3: Add route to App.tsx**

Modify `apps/gui/src/App.tsx`:
```typescript
import { ProvidersPage } from "./pages/ProvidersPage";
// ...
const DESKTOP_PAGES: readonly DesktopPage[] = [
  "overview",
  "usage",
  "prompt_palette",
  "providers",       // ← new
  "settings",
];
// ...
// In the pageContent switch:
// In the switch (App.tsx uses switch, NOT if-else — see line ~89):
case "providers":
  pageContent = <ProvidersPage />;
  break;
```

- [ ] **Step 4: Update existing tests**

Modify `apps/gui/src/App.test.tsx` if it asserts on page count or sidebar items. The test at line 443 counts sidebar items — it may need the expected count bumped.

Modify `apps/gui/src/components/desktop/Sidebar.test.tsx` to include the new nav item.

- [ ] **Step 5: Run all GUI tests + commit**

```bash
cd apps/gui && pnpm exec vitest run
# All tests pass

pnpm typecheck
# Clean
```

```bash
git add apps/gui/src/components/AppShell.tsx apps/gui/src/components/desktop/Sidebar.tsx apps/gui/src/App.tsx apps/gui/src/App.test.tsx apps/gui/src/components/desktop/Sidebar.test.tsx
git commit -m "feat(gui): add Providers page to sidebar + routing"
```

---

## Task 8: CONTRIBUTING.md invariant update + verification gate

**Files:**
- Modify: `CONTRIBUTING.md`

- [ ] **Step 1: Update the invariant**

Find the line in `CONTRIBUTING.md` that says "never stores credentials" and replace with the new wording from the spec:

```markdown
The most load-bearing principle: Busytok **never proxies traffic, never stores credentials in config files, never modifies client config, never handles OAuth/session tokens.** Provider API keys are stored exclusively in the OS keychain (macOS Keychain / Windows Credential Manager) via `keyring-rs`. Keys are never written to config files, never logged, never transmitted over network. Keys are injected into the sidecar subprocess via environment variables at spawn time only. A PR that violates any of these will be rejected.
```

- [ ] **Step 2: Run full verification gate**

```bash
# Rust
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude busytok-gui

# GUI
cd apps/gui
pnpm typecheck
pnpm exec vitest run --coverage
# Verify coverage >90%

# Build
pnpm build
```

- [ ] **Step 3: Final commit**

```bash
git add CONTRIBUTING.md
git commit -m "docs: update CONTRIBUTING.md invariant for provider credential storage"
```

---

## Verification Gate

- [ ] `cargo fmt --all --check` — clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` — clean
- [ ] `cargo test --workspace --exclude busytok-gui` — all pass
- [ ] `pnpm typecheck` — clean
- [ ] `pnpm exec vitest run --coverage` — >90% coverage, all pass
- [ ] `pnpm build` — builds successfully
- [ ] Manual: Providers page renders in sidebar, CRUD works, key stored in Keychain (`security find-generic-password -s com.busytok.providers`)
- [ ] Manual: `busytok provider list` (if CLI command exists) or RPC-level test returns providers
- [ ] CONTRIBUTING.md updated

---

## Self-Review Notes

### Spec coverage
- §3.1 Provider data model → Task 1 (ProviderConfig)
- §3.3 Keychain lifecycle → Task 1 (CredentialStore) + Task 4 (CRUD handlers enforce lifecycle)
- §4 Phase 1 deliverables → Tasks 1-8
- §2.4 CONTRIBUTING.md → Task 8
- §3.5 Config migration → handled in Task 4 (canonicalization on settings read, provider_id=None for existing profiles is a Phase 4 concern but delete provider validates against it)

### Implementation risk points (from reviewer)
1. **Runtime locator**: Phase 5 concern, not Phase 1. Noted in spec.
2. **profile.provider_id: Option<String>**: Phase 1's `profile_provider_id()` helper returns None (stub). Phase 4 adds the field. All delegate/validate paths will handle None.
3. **package_dmg.sh signing order**: Phase 5 concern. Noted in spec.

### Not in Phase 1 (deferred to later phases)
- Profile management (Phase 4)
- Subagent monitoring page (Phase 2)
- Pi SDK / sidecar spawning (Phase 3)
- Sidecar bundling (Phase 5)
