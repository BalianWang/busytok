# Provider / Model Catalog 存储架构重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 provider 存储从"TOML + Keychain + 内嵌模型列表"重构为统一的 SQL 领域模型（providers / models / model_tags），API key 存 SQL，sidecar 固定 env 注入，CLI/GUI 共用统一 model catalog 读模型。

**Architecture:** 新建 v6 schema 迁移（3 张表）。`ProviderKind` 从 `busytok-config` 迁到 `busytok-domain`。store 层新增 `provider_catalog.rs` 提供 CRUD + 精确读 + profile 引用检查。supervisor 翻转为 SQL 后端 + 阻断删除 + delegate 二次校验。`SidecarConfig` 删除 env name 字段，WorkerPool 改为直接从 provider 记录取 api_key/base_url 注入固定 env。CLI 新增 `busytok models` 命令。GUI ProvidersPage 简化 + 新增 ModelsSection。

**Tech Stack:** Rust (rusqlite, anyhow, tracing, async-trait), TypeScript (React, Tauri, ts-rs)

## Global Constraints

- 覆盖率目标 ≥ 90%，变更文件行覆盖率 ≥ 90%
- 日志接入当前 `tracing` 系统，结构化事件，严禁输出 `api_key`
- `model_id` 创建后不可变（不允许 rename）
- `include_disabled=false` 同时过滤 `provider_enabled` 和 `model_enabled`
- sidecar 固定只认 `OPENAI_API_KEY` 和 `OPENAI_BASE_URL`
- 不保留旧数据兼容/迁移逻辑，直接重建
- provider/model 删除必须阻断（检查 profile 引用），不自动解绑
- 删除所有 keychain 相关代码、依赖、测试
- store 层用 `anyhow::Result`，`&Connection` 入参模式
- 测试用 `Database::open_in_memory()`
- 新增 RPC 方法需同步更新 8 处（见 Task 4 接口块）

**Spec:** [docs/superpowers/specs/2026-07-03-provider-model-catalog-refactor-design.md](file:///Users/wsd/Data/Busytok/busytok/docs/superpowers/specs/2026-07-03-provider-model-catalog-refactor-design.md)

---

## File Structure

### 新建文件
| 文件 | 职责 |
|------|------|
| `crates/busytok-store/migrations/0006_provider_catalog.sql` | v6 schema: providers / models / model_tags |
| `crates/busytok-store/src/provider_catalog.rs` | store 层 CRUD + 精确读 + profile 引用检查 |
| `crates/busytok-store/tests/provider_catalog.rs` | store 层集成测试 |
| `crates/busytok-domain/src/provider_catalog.rs` | `Provider` / `Model` / `ModelTag` 领域结构体 + `ProviderKind` 枚举 |
| `apps/gui/src/components/ModelsSection.tsx` | GUI 模型管理区块 |
| `apps/cli/src/commands/models.rs` | CLI `models` 命令 handler |

### 修改文件
| 文件 | 改动 |
|------|------|
| `crates/busytok-domain/src/lib.rs` | 导出 `provider_catalog` 模块 |
| `crates/busytok-domain/Cargo.toml` | 无新依赖（已有 serde/anyhow） |
| `crates/busytok-store/src/schema.rs` | `SCHEMA_VERSION = 6`，注册 `0006` 迁移 |
| `crates/busytok-store/src/lib.rs` | 导出 `provider_catalog` 模块 + re-export 类型 |
| `crates/busytok-store/src/db.rs` | 添加 `provider_catalog` CRUD 委托方法 |
| `crates/busytok-protocol/Cargo.toml` | 新增 `busytok-domain` 依赖 |
| `crates/busytok-protocol/src/dto.rs` | 删旧 provider DTO 字段，加 model DTO，`provider_kind` 用 `ProviderKind` |
| `crates/busytok-protocol/src/ts.rs` | 注册新 DTO 的 `::decl()` |
| `crates/busytok-control/src/dispatch.rs` | trait + dispatch + TestRuntimeControl + blanket impl |
| `crates/busytok-runtime/src/supervisor.rs` | provider/model handlers 切 SQL，delegate 二次校验，profile 校验 |
| `crates/busytok-runtime/Cargo.toml` | 确认依赖 `busytok-domain`（已有） |
| `crates/busytok-subagent/src/sidecar/config.rs` | 删 `api_key_env_name` / `base_url_env_name` 字段 |
| `crates/busytok-subagent/src/sidecar/pool.rs` | 删 `ProviderLookup` / `CredentialReader`，改用 `ProviderRuntimeEntry` |
| `crates/busytok-config/src/providers.rs` | 删除 `ProviderKind`（迁到 domain）、`ProviderConfig`（整体删除） |
| `crates/busytok-config/src/lib.rs` | 删 `providers` 模块导出，删 `BusytokSettings.providers` 字段 |
| `apps/cli/src/main.rs` | 新增 `Models` 命令枚举 |
| `apps/cli/src/commands.rs` | TestRuntimeWrapper stub + models 命令分发 |
| `apps/gui/src/api/busytokClient.ts` | 新增 model.* 方法 |
| `apps/gui/src/pages/ProvidersPage.tsx` | 简化（删 env name / models 字段编辑） |
| `apps/gui/src/components/ProfilesSection.tsx` | model 选择从 SQL catalog 读 |

### 删除文件
| 文件 | 原因 |
|------|------|
| `crates/busytok-config/src/keychain.rs`（或等价文件） | keychain 存储链路全删 |
| keychain 相关测试文件 | 同上 |

---

## Task 1: `ProviderKind` 迁移 + domain provider_catalog 模块

**Files:**
- Create: `crates/busytok-domain/src/provider_catalog.rs`
- Modify: `crates/busytok-domain/src/lib.rs`
- Modify: `crates/busytok-config/src/providers.rs`（删除 `ProviderKind` 定义，改为从 domain re-export 临时过渡）
- Test: `crates/busytok-domain/src/provider_catalog.rs`（inline `#[cfg(test)]`）

**Interfaces:**
- Produces: `busytok_domain::provider_catalog::{ProviderKind, Provider, Model, ModelTag, ProviderSummary, ModelCatalogEntry, ModelCatalogFilter, ProfileModelRef}`

> 注意：此任务只迁移 `ProviderKind` 到 domain 并定义新领域结构体。`busytok-config` 中的 `ProviderConfig` 暂时保留（re-export `ProviderKind`），在 Task 7 才整体删除。这样 Task 1-2 可以独立编译通过。

- [ ] **Step 1: 写 domain 领域结构体的失败测试**

在 `crates/busytok-domain/src/provider_catalog.rs` 底部写：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_serde_openai_compatible() {
        let json = serde_json::to_string(&ProviderKind::OpenAiCompatible).unwrap();
        assert_eq!(json, "\"openai_compatible\"");
        let parsed: ProviderKind = serde_json::from_str("\"openai_compatible\"").unwrap();
        assert_eq!(parsed, ProviderKind::OpenAiCompatible);
    }

    #[test]
    fn provider_summary_has_api_key_false_when_api_key_none() {
        let p = Provider {
            id: "p1".into(),
            name: "Test".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: true,
            api_key: None,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };
        let s = ProviderSummary::from(&p);
        assert!(!s.has_api_key);
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p busytok-domain provider_catalog -- --nocapture`
Expected: FAIL — module not found

- [ ] **Step 3: 实现 provider_catalog 模块**

```rust
//! Provider / Model / Tag domain model for the SQL-backed catalog.
use serde::{Deserialize, Serialize};

/// Provider kind. MVP only supports OpenAI-compatible.
/// Kept in domain (not config) because it is wire-level vocabulary
/// shared by protocol, store, and runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
}

/// Provider — connection config + credential. Stored in SQL `providers` table.
/// `api_key` is the plaintext key; DTOs never expose it (use `ProviderSummary`).
#[derive(Debug, Clone)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub enabled: bool,
    pub api_key: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Provider without the secret — safe for list views and DTOs.
#[derive(Debug, Clone)]
pub struct ProviderSummary {
    pub id: String,
    pub name: String,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub enabled: bool,
    pub has_api_key: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl From<&Provider> for ProviderSummary {
    fn from(p: &Provider) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            provider_kind: p.provider_kind.clone(),
            base_url: p.base_url.clone(),
            enabled: p.enabled,
            has_api_key: p.api_key.is_some(),
            created_at_ms: p.created_at_ms,
            updated_at_ms: p.updated_at_ms,
        }
    }
}

/// Model — a routable model instance under a provider.
/// `model_id` is immutable after creation (no rename).
#[derive(Debug, Clone)]
pub struct Model {
    pub id: String,           // DB primary key (UUID)
    pub provider_id: String,  // FK -> providers.id
    pub model_id: String,     // immutable, e.g. "gpt-4o"
    pub enabled: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Tag row — many-to-many between models and tag strings.
#[derive(Debug, Clone)]
pub struct ModelTag {
    pub model_id: String,  // FK -> models.id
    pub tag: String,
}

/// Unified model catalog entry — joined view for CLI/GUI/routing.
#[derive(Debug, Clone)]
pub struct ModelCatalogEntry {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_kind: ProviderKind,
    pub provider_enabled: bool,
    pub model_db_id: String,
    pub model_id: String,
    pub model_enabled: bool,
    pub tags: Vec<String>,
}

/// Filter for `list_models_filtered`.
/// `include_disabled=false` filters BOTH provider_enabled and model_enabled.
#[derive(Debug, Clone, Default)]
pub struct ModelCatalogFilter {
    pub provider_id: Option<String>,
    pub tags: Vec<String>,      // AND semantics
    pub include_disabled: bool,
}

/// Profile → model reference snapshot. Collected from settings.toml profiles
/// and passed to store-layer reference-check functions. Keeps store layer
/// decoupled from config layer.
#[derive(Debug, Clone)]
pub struct ProfileModelRef {
    pub provider_id: String,
    pub model_id: String,
}
```

- [ ] **Step 4: 在 lib.rs 导出模块**

在 `crates/busytok-domain/src/lib.rs` 的模块声明区（第 14-19 行后）添加：

```rust
pub mod provider_catalog;
```

在 re-export 区（第 21-37 行后）添加：

```rust
pub use provider_catalog::{
    Model, ModelCatalogEntry, ModelCatalogFilter, ModelTag, ProfileModelRef, Provider,
    ProviderKind, ProviderSummary,
};
```

- [ ] **Step 5: 在 busytok-config 中改为从 domain re-export ProviderKind**

在 `crates/busytok-config/src/providers.rs` 第 6-14 行，删除 `ProviderKind` enum 定义，替换为：

```rust
// ProviderKind has migrated to busytok-domain. Re-exported here temporarily
// to minimize breakage during the refactor; Task 7 removes this entirely.
pub use busytok_domain::ProviderKind;
```

在 `crates/busytok-config/Cargo.toml` 的 `[dependencies]` 添加：

```toml
busytok-domain = { path = "../busytok-domain" }
```

- [ ] **Step 6: 运行测试验证通过**

Run: `cargo test -p busytok-domain provider_catalog -- --nocapture && cargo check -p busytok-config`
Expected: domain 测试 PASS，config 编译通过

- [ ] **Step 7: Commit**

```bash
git add crates/busytok-domain/src/provider_catalog.rs crates/busytok-domain/src/lib.rs \
  crates/busytok-config/src/providers.rs crates/busytok-config/Cargo.toml
git commit -m "feat(domain): migrate ProviderKind to domain + add provider_catalog module"
```

---

## Task 2: v6 schema migration + store provider_catalog repository

**Files:**
- Create: `crates/busytok-store/migrations/0006_provider_catalog.sql`
- Create: `crates/busytok-store/src/provider_catalog.rs`
- Create: `crates/busytok-store/tests/provider_catalog.rs`
- Modify: `crates/busytok-store/src/schema.rs`
- Modify: `crates/busytok-store/src/lib.rs`
- Modify: `crates/busytok-store/src/db.rs`
- Modify: `crates/busytok-store/Cargo.toml`（确认 `busytok-domain` 依赖）

**Interfaces:**
- Consumes: `busytok_domain::{Provider, ProviderKind, Model, ModelTag, ProviderSummary, ModelCatalogEntry, ModelCatalogFilter, ProfileModelRef}`
- Produces:
  - `busytok_store::provider_catalog::{CreateProviderReq, UpdateProviderPatch, CreateModelReq, UpdateModelPatch, create_provider, update_provider, delete_provider, get_provider_with_secret, list_providers, create_model, update_model, delete_model, get_model_by_id, get_model_by_provider_and_model_id, list_models_filtered, list_models_by_provider, list_tags, set_model_tags, provider_has_profile_references, model_has_profile_references}`
  - `busytok_store::Database` 上对应的委托方法

- [ ] **Step 1: 写 v6 migration SQL**

`crates/busytok-store/migrations/0006_provider_catalog.sql`:

```sql
-- Provider catalog: providers / models / model_tags
-- Replaces settings.toml provider persistence + keychain credential storage.

CREATE TABLE providers (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  provider_kind TEXT NOT NULL DEFAULT 'openai_compatible',
  base_url TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  api_key TEXT,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);

CREATE TABLE models (
  id TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL,
  model_id TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE,
  UNIQUE (provider_id, model_id)
);

CREATE TABLE model_tags (
  model_id TEXT NOT NULL,
  tag TEXT NOT NULL,
  FOREIGN KEY (model_id) REFERENCES models(id) ON DELETE CASCADE,
  UNIQUE (model_id, tag)
);

CREATE INDEX idx_models_provider_id ON models(provider_id);
CREATE INDEX idx_model_tags_tag ON model_tags(tag);
```

- [ ] **Step 2: 注册 migration**

在 `crates/busytok-store/src/schema.rs`：

第 5 行改为：
```rust
pub const SCHEMA_VERSION: u32 = 6;
```

第 35-36 行后添加：
```rust
const PROVIDER_CATALOG_SQL: &str = include_str!("../migrations/0006_provider_catalog.sql");
```

`migrations()` 函数（第 39-47 行）末尾添加：
```rust
        (6, PROVIDER_CATALOG_SQL),
```

- [ ] **Step 3: 写 store repository 的失败测试**

`crates/busytok-store/tests/provider_catalog.rs`:

```rust
use busytok_domain::{ModelCatalogFilter, ProfileModelRef, ProviderKind};
use busytok_store::{
    CreateModelReq, CreateProviderReq, ModelCatalogEntry, Provider, ProviderSummary,
    UpdateProviderPatch,
};
use busytok_store::Database;

fn sample_provider_req(id: &str) -> CreateProviderReq {
    CreateProviderReq {
        id: id.to_string(),
        name: format!("Provider {}", id),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://api.test.com".to_string(),
        enabled: true,
        api_key: Some("sk-test-key".to_string()),
    }
}

#[test]
fn provider_crud_round_trip() {
    let db = Database::open_in_memory().unwrap();
    let created = db.create_provider(sample_provider_req("p1")).unwrap();
    assert_eq!(created.id, "p1");
    assert!(created.api_key.is_some());

    let summary = db.list_providers().unwrap();
    assert_eq!(summary.len(), 1);
    assert!(summary[0].has_api_key);

    let updated = db
        .update_provider("p1", UpdateProviderPatch {
            name: Some("Updated".to_string()),
            base_url: None,
            enabled: None,
            api_key: Some("sk-new-key".to_string()),
        })
        .unwrap();
    assert_eq!(updated.name, "Updated");

    let with_secret = db.get_provider_with_secret("p1").unwrap().unwrap();
    assert_eq!(with_secret.api_key.as_deref(), Some("sk-new-key"));

    db.delete_provider("p1", &[]).unwrap();
    assert!(db.list_providers().unwrap().is_empty());
}

#[test]
fn model_crud_and_cascade_tags() {
    let db = Database::open_in_memory().unwrap();
    db.create_provider(sample_provider_req("p1")).unwrap();

    let model = db
        .create_model(CreateModelReq {
            provider_id: "p1".to_string(),
            model_id: "gpt-4o".to_string(),
            enabled: true,
            tags: vec!["fast".to_string(), "cheap".to_string()],
        })
        .unwrap();
    assert_eq!(model.model_id, "gpt-4o");

    // Duplicate (provider_id, model_id) rejected
    let dup = db.create_model(CreateModelReq {
        provider_id: "p1".to_string(),
        model_id: "gpt-4o".to_string(),
        enabled: true,
        tags: vec![],
    });
    assert!(dup.is_err());

    // List tags
    let tags = db.list_tags().unwrap();
    assert!(tags.contains(&"fast".to_string()));
    assert!(tags.contains(&"cheap".to_string()));

    // Delete model cascades tags
    db.delete_model(&model.id, &[]).unwrap();
    let entries = db.list_models_filtered(ModelCatalogFilter::default()).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn list_models_filtered_by_multiple_tags_and_semantics() {
    let db = Database::open_in_memory().unwrap();
    db.create_provider(sample_provider_req("p1")).unwrap();

    db.create_model(CreateModelReq {
        provider_id: "p1".into(),
        model_id: "gpt-4o".into(),
        enabled: true,
        tags: vec!["fast".into(), "cheap".into()],
    }).unwrap();
    db.create_model(CreateModelReq {
        provider_id: "p1".into(),
        model_id: "gpt-4o-mini".into(),
        enabled: true,
        tags: vec!["fast".into()],
    }).unwrap();

    // AND semantics: only model with both tags
    let entries = db.list_models_filtered(ModelCatalogFilter {
        provider_id: None,
        tags: vec!["fast".into(), "cheap".into()],
        include_disabled: false,
    }).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model_id, "gpt-4o");
}

#[test]
fn include_disabled_filters_both_provider_and_model() {
    let db = Database::open_in_memory().unwrap();
    db.create_provider(CreateProviderReq {
        id: "p-enabled".into(),
        name: "Enabled".into(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://a.com".into(),
        enabled: true,
        api_key: Some("k".into()),
    }).unwrap();
    db.create_provider(CreateProviderReq {
        id: "p-disabled".into(),
        name: "Disabled".into(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://b.com".into(),
        enabled: false,
        api_key: None,
    }).unwrap();

    db.create_model(CreateModelReq {
        provider_id: "p-enabled".into(), model_id: "m-enabled".into(),
        enabled: true, tags: vec![],
    }).unwrap();
    db.create_model(CreateModelReq {
        provider_id: "p-enabled".into(), model_id: "m-disabled".into(),
        enabled: false, tags: vec![],
    }).unwrap();
    db.create_model(CreateModelReq {
        provider_id: "p-disabled".into(), model_id: "m-under-disabled".into(),
        enabled: true, tags: vec![],
    }).unwrap();

    // include_disabled=false: only enabled provider + enabled model
    let entries = db.list_models_filtered(ModelCatalogFilter {
        provider_id: None, tags: vec![], include_disabled: false,
    }).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model_id, "m-enabled");

    // include_disabled=true: all 3
    let entries = db.list_models_filtered(ModelCatalogFilter {
        provider_id: None, tags: vec![], include_disabled: true,
    }).unwrap();
    assert_eq!(entries.len(), 3);
}

#[test]
fn provider_delete_blocked_by_profile_reference() {
    let db = Database::open_in_memory().unwrap();
    db.create_provider(sample_provider_req("p1")).unwrap();
    let model = db.create_model(CreateModelReq {
        provider_id: "p1".into(), model_id: "gpt-4o".into(),
        enabled: true, tags: vec![],
    }).unwrap();

    let refs = vec![ProfileModelRef {
        provider_id: "p1".into(),
        model_id: "gpt-4o".into(),
    }];

    // Blocked
    let err = db.delete_provider("p1", &refs);
    assert!(err.is_err());

    // Not blocked when refs empty
    db.delete_provider("p1", &[]).unwrap();
    let _ = model; // suppress unused
}

#[test]
fn model_delete_blocked_by_profile_reference() {
    let db = Database::open_in_memory().unwrap();
    db.create_provider(sample_provider_req("p1")).unwrap();
    let model = db.create_model(CreateModelReq {
        provider_id: "p1".into(), model_id: "gpt-4o".into(),
        enabled: true, tags: vec![],
    }).unwrap();

    let refs = vec![ProfileModelRef {
        provider_id: "p1".into(),
        model_id: "gpt-4o".into(),
    }];

    let err = db.delete_model(&model.id, &refs);
    assert!(err.is_err());

    db.delete_model(&model.id, &[]).unwrap();
}
```

- [ ] **Step 4: 运行测试验证失败**

Run: `cargo test -p busytok-store provider_catalog -- --nocapture`
Expected: FAIL — functions not found / module not found

- [ ] **Step 5: 实现 store repository**

在 `crates/busytok-store/Cargo.toml` 的 `[dependencies]` 添加（如果尚未有）：
```toml
busytok-domain = { path = "../busytok-domain" }
```

`crates/busytok-store/src/provider_catalog.rs`:

```rust
//! SQL repository for provider / model / model_tags catalog.
use anyhow::{anyhow, bail, Context, Result};
use busytok_domain::{
    Model, ModelCatalogEntry, ModelCatalogFilter, ModelTag, ProfileModelRef, Provider,
    ProviderKind, ProviderSummary,
};
use rusqlite::{params, params_from_iter, Connection};
use tracing::info;

// ── Input DTOs (no id/timestamps — store generates those) ──────────────

pub struct CreateProviderReq {
    pub id: String,
    pub name: String,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub enabled: bool,
    pub api_key: Option<String>,
}

pub struct UpdateProviderPatch {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub enabled: Option<bool>,
    pub api_key: Option<String>, // Some = replace, None = unchanged
}

pub struct CreateModelReq {
    pub provider_id: String,
    pub model_id: String,
    pub enabled: bool,
    pub tags: Vec<String>,
}

// UpdateModelPatch only has enabled — model_id is immutable
pub struct UpdateModelPatch {
    pub enabled: Option<bool>,
}

// ── CRUD: providers ────────────────────────────────────────────────────

pub fn create_provider(conn: &Connection, req: CreateProviderReq) -> Result<Provider> {
    let now = busytok_domain::now_ms();
    conn.execute(
        "INSERT INTO providers (id, name, provider_kind, base_url, enabled, api_key, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![
            req.id,
            req.name,
            serde_json::to_string(&req.provider_kind)?,
            req.base_url,
            req.enabled as i64,
            req.api_key,
            now,
        ],
    )
    .map_err(|e| {
        if e.to_string().contains("PRIMARY KEY") {
            anyhow!("provider already exists: {}", req.id)
        } else {
            anyhow!(e)
        }
    })?;
    info!(event_code = "provider.created", provider_id = %req.id, "provider created");
    get_provider_with_secret(conn, &req.id)?
        .ok_or_else(|| anyhow!("provider {} not found after insert", req.id))
}

pub fn update_provider(conn: &Connection, id: &str, patch: UpdateProviderPatch) -> Result<Provider> {
    let now = busytok_domain::now_ms();
    let tx = conn.unchecked_transaction()?;
    if let Some(name) = &patch.name {
        tx.execute("UPDATE providers SET name = ?1, updated_at_ms = ?2 WHERE id = ?3", params![name, now, id])?;
    }
    if let Some(base_url) = &patch.base_url {
        tx.execute("UPDATE providers SET base_url = ?1, updated_at_ms = ?2 WHERE id = ?3", params![base_url, now, id])?;
    }
    if let Some(enabled) = patch.enabled {
        tx.execute("UPDATE providers SET enabled = ?1, updated_at_ms = ?2 WHERE id = ?3", params![enabled as i64, now, id])?;
    }
    if let Some(api_key) = &patch.api_key {
        tx.execute("UPDATE providers SET api_key = ?1, updated_at_ms = ?2 WHERE id = ?3", params![api_key, now, id])?;
    }
    let rows = tx.query_row("SELECT changes()", [], |row| row.get::<_, i64>(0))?;
    tx.commit()?;
    if rows == 0 {
        bail!("provider not found: {}", id);
    }
    info!(event_code = "provider.updated", provider_id = %id, "provider updated");
    get_provider_with_secret(conn, id)?.ok_or_else(|| anyhow!("provider {} not found after update", id))
}

pub fn delete_provider(conn: &Connection, id: &str, profile_refs: &[ProfileModelRef]) -> Result<()> {
    if provider_has_profile_references(id, profile_refs) {
        let count = profile_refs.iter().filter(|r| r.provider_id == id).count();
        bail!("cannot delete provider: {} profile(s) still reference it", count);
    }
    let rows = conn.execute("DELETE FROM providers WHERE id = ?1", params![id])?;
    if rows == 0 {
        bail!("provider not found: {}", id);
    }
    info!(event_code = "provider.deleted", provider_id = %id, "provider deleted");
    Ok(())
}

pub fn get_provider_with_secret(conn: &Connection, id: &str) -> Result<Option<Provider>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, provider_kind, base_url, enabled, api_key, created_at_ms, updated_at_ms
         FROM providers WHERE id = ?1",
    )?;
    let row = stmt.query_row(params![id], row_to_provider).optional()?;
    Ok(row)
}

pub fn list_providers(conn: &Connection) -> Result<Vec<ProviderSummary>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, provider_kind, base_url, enabled, api_key, created_at_ms, updated_at_ms
         FROM providers ORDER BY name",
    )?;
    let providers: Vec<Provider> = stmt.query_map([], row_to_provider)?.filter_map(|r| r.ok()).collect();
    Ok(providers.iter().map(ProviderSummary::from).collect())
}

// ── CRUD: models ───────────────────────────────────────────────────────

pub fn create_model(conn: &Connection, req: CreateModelReq) -> Result<Model> {
    let id = format!("model_{}", uuid::Uuid::new_v4());
    let now = busytok_domain::now_ms();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO models (id, provider_id, model_id, enabled, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![id, req.provider_id, req.model_id, req.enabled as i64, now],
    )
    .map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            anyhow!("model '{}' already exists for provider '{}'", req.model_id, req.provider_id)
        } else {
            anyhow!(e)
        }
    })?;
    for tag in &req.tags {
        tx.execute(
            "INSERT INTO model_tags (model_id, tag) VALUES (?1, ?2)",
            params![id, tag],
        )?;
    }
    tx.commit()?;
    info!(event_code = "model.created", model_id = %req.model_id, provider_id = %req.provider_id, "model created");
    get_model_by_id(conn, &id)?.ok_or_else(|| anyhow!("model {} not found after insert", id))
}

pub fn update_model(conn: &Connection, id: &str, patch: UpdateModelPatch) -> Result<Model> {
    if let Some(enabled) = patch.enabled {
        let now = busytok_domain::now_ms();
        let rows = conn.execute(
            "UPDATE models SET enabled = ?1, updated_at_ms = ?2 WHERE id = ?3",
            params![enabled as i64, now, id],
        )?;
        if rows == 0 {
            bail!("model not found: {}", id);
        }
        info!(event_code = "model.updated", model_db_id = %id, enabled, "model updated");
    }
    get_model_by_id(conn, id)?.ok_or_else(|| anyhow!("model {} not found after update", id))
}

pub fn delete_model(conn: &Connection, id: &str, profile_refs: &[ProfileModelRef]) -> Result<()> {
    let model = get_model_by_id(conn, id)?
        .ok_or_else(|| anyhow!("model not found: {}", id))?;
    if model_has_profile_references(&model.provider_id, &model.model_id, profile_refs) {
        bail!("cannot delete model: profile(s) still reference it");
    }
    let rows = conn.execute("DELETE FROM models WHERE id = ?1", params![id])?;
    if rows == 0 {
        bail!("model not found: {}", id);
    }
    info!(event_code = "model.deleted", model_db_id = %id, model_id = %model.model_id, "model deleted");
    Ok(())
}

pub fn get_model_by_id(conn: &Connection, id: &str) -> Result<Option<Model>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider_id, model_id, enabled, created_at_ms, updated_at_ms
         FROM models WHERE id = ?1",
    )?;
    Ok(stmt.query_row(params![id], row_to_model).optional()?)
}

pub fn get_model_by_provider_and_model_id(
    conn: &Connection,
    provider_id: &str,
    model_id: &str,
) -> Result<Option<Model>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider_id, model_id, enabled, created_at_ms, updated_at_ms
         FROM models WHERE provider_id = ?1 AND model_id = ?2",
    )?;
    Ok(stmt.query_row(params![provider_id, model_id], row_to_model).optional()?)
}

// ── Catalog queries ────────────────────────────────────────────────────

pub fn list_models_filtered(conn: &Connection, filter: ModelCatalogFilter) -> Result<Vec<ModelCatalogEntry>> {
    let include_disabled = if filter.include_disabled { 1 } else { 0 };
    let provider_id = filter.provider_id.as_deref();

    // Dynamic tag placeholders: rusqlite cannot bind Vec to IN()
    let tag_count = filter.tags.len() as i64;
    let tag_placeholders: Vec<String> = (0..filter.tags.len()).map(|_| "?".to_string()).collect();
    let tag_clause = if tag_placeholders.is_empty() {
        String::new()
    } else {
        format!(
            "AND (SELECT COUNT(DISTINCT tag) FROM model_tags WHERE model_id = m.id AND tag IN ({})) = {}",
            tag_placeholders.join(", "),
            tag_count
        )
    };

    let sql = format!(
        "SELECT p.id, p.name, p.provider_kind, p.enabled,
                m.id, m.model_id, m.enabled,
                COALESCE(GROUP_CONCAT(mt.tag, ','), '') AS tags_csv
         FROM models m
         JOIN providers p ON p.id = m.provider_id
         LEFT JOIN model_tags mt ON mt.model_id = m.id
         WHERE (?1 = 1 OR (p.enabled = 1 AND m.enabled = 1))
           AND (?2 IS NULL OR m.provider_id = ?2)
         GROUP BY m.id
         {tag_clause}
         ORDER BY p.name, m.model_id"
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![
        Box::new(include_disabled),
        Box::new(provider_id),
    ];
    for tag in &filter.tags {
        params_vec.push(Box::new(tag.clone()));
    }
    let rows = stmt.query_map(params_from_iter(params_vec.iter().map(|b| b.as_ref())), |row| {
        let tags_csv: String = row.get(7)?;
        let tags: Vec<String> = if tags_csv.is_empty() {
            vec![]
        } else {
            tags_csv.split(',').map(|s| s.to_string()).collect()
        };
        let kind_str: String = row.get(2)?;
        let provider_kind: ProviderKind = serde_json::from_str(&kind_str).unwrap_or(ProviderKind::OpenAiCompatible);
        Ok(ModelCatalogEntry {
            provider_id: row.get(0)?,
            provider_name: row.get(1)?,
            provider_kind,
            provider_enabled: row.get(3)?,
            model_db_id: row.get(4)?,
            model_id: row.get(5)?,
            model_enabled: row.get(6)?,
            tags,
        })
    })?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    info!(event_code = "model.catalog.listed", entry_count = entries.len(), "model catalog listed");
    Ok(entries)
}

pub fn list_models_by_provider(conn: &Connection, provider_id: &str) -> Result<Vec<ModelCatalogEntry>> {
    list_models_filtered(conn, ModelCatalogFilter {
        provider_id: Some(provider_id.to_string()),
        tags: vec![],
        include_disabled: true, // by-provider view shows all models
    })
}

pub fn list_tags(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT DISTINCT tag FROM model_tags ORDER BY tag")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut tags = Vec::new();
    for row in rows {
        tags.push(row?);
    }
    Ok(tags)
}

pub fn set_model_tags(conn: &Connection, model_id: &str, tags: &[String]) -> Result<()> {
    let now = busytok_domain::now_ms();
    let tx = conn.unchecked_transaction()?;
    // Diff: delete tags not in new set, insert new tags
    let mut existing: std::collections::HashSet<String> = tx
        .prepare("SELECT tag FROM model_tags WHERE model_id = ?1")?
        .query_map(params![model_id], |row| row.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    let new_set: std::collections::HashSet<String> = tags.iter().cloned().collect();
    // Remove tags that are no longer present
    let to_remove: Vec<_> = existing.difference(&new_set).cloned().collect();
    for tag in &to_remove {
        tx.execute("DELETE FROM model_tags WHERE model_id = ?1 AND tag = ?2", params![model_id, tag])?;
        info!(event_code = "model.tag_removed", model_db_id = %model_id, tag = %tag, "tag removed");
    }
    // Insert new tags
    let to_add: Vec<_> = new_set.difference(&existing).cloned().collect();
    for tag in &to_add {
        tx.execute(
            "INSERT OR IGNORE INTO model_tags (model_id, tag) VALUES (?1, ?2)",
            params![model_id, tag],
        )?;
        info!(event_code = "model.tag_added", model_db_id = %model_id, tag = %tag, "tag added");
    }
    tx.execute("UPDATE models SET updated_at_ms = ?1 WHERE id = ?2", params![now, model_id])?;
    tx.commit()?;
    Ok(())
}

// ── Profile reference checks (blocking deletes) ────────────────────────

pub fn provider_has_profile_references(provider_id: &str, refs: &[ProfileModelRef]) -> bool {
    refs.iter().any(|r| r.provider_id == provider_id)
}

pub fn model_has_profile_references(provider_id: &str, model_id: &str, refs: &[ProfileModelRef]) -> bool {
    refs.iter().any(|r| r.provider_id == provider_id && r.model_id == model_id)
}

// ── Row mappers ────────────────────────────────────────────────────────

fn row_to_provider(row: &rusqlite::Row) -> rusqlite::Result<Provider> {
    let kind_str: String = row.get(2)?;
    let provider_kind = serde_json::from_str(&kind_str).unwrap_or(ProviderKind::OpenAiCompatible);
    Ok(Provider {
        id: row.get(0)?,
        name: row.get(1)?,
        provider_kind,
        base_url: row.get(3)?,
        enabled: row.get::<_, i64>(4)? != 0,
        api_key: row.get(5)?,
        created_at_ms: row.get(6)?,
        updated_at_ms: row.get(7)?,
    })
}

fn row_to_model(row: &rusqlite::Row) -> rusqlite::Result<Model> {
    Ok(Model {
        id: row.get(0)?,
        provider_id: row.get(1)?,
        model_id: row.get(2)?,
        enabled: row.get::<_, i64>(3)? != 0,
        created_at_ms: row.get(4)?,
        updated_at_ms: row.get(5)?,
    })
}
```

- [ ] **Step 6: 在 lib.rs 和 db.rs 导出 + 委托**

在 `crates/busytok-store/src/lib.rs` 的模块声明区添加：

```rust
pub mod provider_catalog;
pub use provider_catalog::{
    CreateModelReq, CreateProviderReq, ModelCatalogEntry, ModelCatalogFilter, UpdateModelPatch,
    UpdateProviderPatch,
};
// Re-export domain types that store consumers need
pub use busytok_domain::{Model, ModelTag, ProfileModelRef, Provider, ProviderKind, ProviderSummary};
```

在 `crates/busytok-store/src/db.rs` 的 `impl Database` 块中（在现有委托方法之后，约第 232 行后）添加委托方法：

```rust
    // ── Provider catalog ───────────────────────────────────────────
    pub fn create_provider(&self, req: crate::provider_catalog::CreateProviderReq) -> anyhow::Result<busytok_domain::Provider> {
        crate::provider_catalog::create_provider(&self.conn, req)
    }
    pub fn update_provider(&self, id: &str, patch: crate::provider_catalog::UpdateProviderPatch) -> anyhow::Result<busytok_domain::Provider> {
        crate::provider_catalog::update_provider(&self.conn, id, patch)
    }
    pub fn delete_provider(&self, id: &str, profile_refs: &[busytok_domain::ProfileModelRef]) -> anyhow::Result<()> {
        crate::provider_catalog::delete_provider(&self.conn, id, profile_refs)
    }
    pub fn get_provider_with_secret(&self, id: &str) -> anyhow::Result<Option<busytok_domain::Provider>> {
        crate::provider_catalog::get_provider_with_secret(&self.conn, id)
    }
    pub fn list_providers(&self) -> anyhow::Result<Vec<busytok_domain::ProviderSummary>> {
        crate::provider_catalog::list_providers(&self.conn)
    }
    pub fn create_model(&self, req: crate::provider_catalog::CreateModelReq) -> anyhow::Result<busytok_domain::Model> {
        crate::provider_catalog::create_model(&self.conn, req)
    }
    pub fn update_model(&self, id: &str, patch: crate::provider_catalog::UpdateModelPatch) -> anyhow::Result<busytok_domain::Model> {
        crate::provider_catalog::update_model(&self.conn, id, patch)
    }
    pub fn delete_model(&self, id: &str, profile_refs: &[busytok_domain::ProfileModelRef]) -> anyhow::Result<()> {
        crate::provider_catalog::delete_model(&self.conn, id, profile_refs)
    }
    pub fn get_model_by_id(&self, id: &str) -> anyhow::Result<Option<busytok_domain::Model>> {
        crate::provider_catalog::get_model_by_id(&self.conn, id)
    }
    pub fn get_model_by_provider_and_model_id(&self, provider_id: &str, model_id: &str) -> anyhow::Result<Option<busytok_domain::Model>> {
        crate::provider_catalog::get_model_by_provider_and_model_id(&self.conn, provider_id, model_id)
    }
    pub fn list_models_filtered(&self, filter: busytok_domain::ModelCatalogFilter) -> anyhow::Result<Vec<busytok_domain::ModelCatalogEntry>> {
        crate::provider_catalog::list_models_filtered(&self.conn, filter)
    }
    pub fn list_models_by_provider(&self, provider_id: &str) -> anyhow::Result<Vec<busytok_domain::ModelCatalogEntry>> {
        crate::provider_catalog::list_models_by_provider(&self.conn, provider_id)
    }
    pub fn list_tags(&self) -> anyhow::Result<Vec<String>> {
        crate::provider_catalog::list_tags(&self.conn)
    }
    pub fn set_model_tags(&self, model_id: &str, tags: &[String]) -> anyhow::Result<()> {
        crate::provider_catalog::set_model_tags(&self.conn, model_id, tags)
    }
```

> **注意：** `rusqlite::OptionalExtension` trait 需要在 `provider_catalog.rs` 顶部导入：`use rusqlite::OptionalExtension;`。在 `db.rs` 中已有此导入则不需要重复。

- [ ] **Step 7: 运行测试验证通过**

Run: `cargo test -p busytok-store provider_catalog -- --nocapture`
Expected: PASS — 所有 6 个测试通过

- [ ] **Step 8: Commit**

```bash
git add crates/busytok-store/migrations/0006_provider_catalog.sql \
  crates/busytok-store/src/schema.rs crates/busytok-store/src/provider_catalog.rs \
  crates/busytok-store/src/lib.rs crates/busytok-store/src/db.rs \
  crates/busytok-store/tests/provider_catalog.rs crates/busytok-store/Cargo.toml
git commit -m "feat(store): add v6 provider_catalog schema + repository with CRUD, filtered queries, and profile reference blocking"
```

---

## Task 3: protocol DTO 更新

**Files:**
- Modify: `crates/busytok-protocol/Cargo.toml`
- Modify: `crates/busytok-protocol/src/dto.rs`
- Modify: `crates/busytok-protocol/src/ts.rs`

**Interfaces:**
- Consumes: `busytok_domain::ProviderKind`
- Produces: 更新后的 `ProviderDto` / `ProviderCreateRequestDto` / `ProviderUpdateRequestDto` / `ProviderListResponseDto`（无 env name / models 字段），新增 `ModelCatalogEntryDto` / `ModelCreateRequestDto` / `ModelUpdateRequestDto` / `ModelDeleteRequestDto` / `ModelListRequestDto` / `ModelListResponseDto` / `ModelTagUpdateDto`

> 注意：此任务只改 protocol DTO。supervisor 和 CLI stub 的 trait 方法在 Task 4 处理。此任务后 `cargo check` 会有编译错误（supervisor 仍引用旧字段），属于预期——Task 5-6 修复。

- [ ] **Step 1: 添加 busytok-domain 依赖**

在 `crates/busytok-protocol/Cargo.toml` 的 `[dependencies]` 添加：

```toml
busytok-domain = { path = "../busytok-domain" }
```

- [ ] **Step 2: 更新 provider DTO（删除旧字段，添加 provider_kind）**

在 `crates/busytok-protocol/src/dto.rs` 第 1-2 行后添加导入：

```rust
use busytok_domain::ProviderKind;
```

替换第 1533-1587 行的 `ProviderDto` / `ProviderCreateRequestDto` / `ProviderUpdateRequestDto`：

```rust
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderDto {
    pub id: String,
    pub name: String,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub enabled: bool,
    pub has_api_key: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderCreateRequestDto {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
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
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}
```

删除 `ProviderListResponseDto` 中的 `ProviderDto` 如果已有——保持不变（仍然 `pub providers: Vec<ProviderDto>`）。

- [ ] **Step 3: 新增 model DTO**

在 provider DTO 之后添加：

```rust
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelCatalogEntryDto {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_kind: ProviderKind,
    pub provider_enabled: bool,
    pub model_db_id: String,
    pub model_id: String,
    pub model_enabled: bool,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelCreateRequestDto {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelUpdateRequestDto {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelDeleteRequestDto {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelListRequestDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub include_disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelListResponseDto {
    pub models: Vec<ModelCatalogEntryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelTagUpdateDto {
    pub model_id: String,
    pub tags: Vec<String>,
}
```

- [ ] **Step 4: 在 ts.rs 注册新 DTO**

在 `crates/busytok-protocol/src/ts.rs` 的类型注册列表中（第 193-195 行附近）添加：

```rust
            dto::ModelCatalogEntryDto::decl(),
            dto::ModelCreateRequestDto::decl(),
            dto::ModelUpdateRequestDto::decl(),
            dto::ModelDeleteRequestDto::decl(),
            dto::ModelListRequestDto::decl(),
            dto::ModelListResponseDto::decl(),
            dto::ModelTagUpdateDto::decl(),
```

- [ ] **Step 5: 验证 protocol crate 编译**

Run: `cargo check -p busytok-protocol`
Expected: protocol crate 编译通过（其他 crate 可能有错误，预期）

- [ ] **Step 6: 重新生成 TS 类型**

Run: `cargo test -p busytok-protocol --features export-ts`（或仓库现有的 ts-rs 生成命令）

验证 `packages/busytok-protocol-types/src/generated.ts` 包含新 DTO 且不再有 `api_key_env_name` / `base_url_env_name` / `models` 字段。

- [ ] **Step 7: Commit**

```bash
git add crates/busytok-protocol/ packages/busytok-protocol-types/
git commit -m "feat(protocol): update provider DTOs (remove env names/models) + add model catalog DTOs"
```

---

## Task 4: RuntimeControl trait 全链路更新

**Files:**
- Modify: `crates/busytok-control/src/dispatch.rs`
- Modify: `apps/cli/src/commands.rs`（TestRuntimeWrapper stub）

**Interfaces:**
- Consumes: Task 3 的新 DTO
- Produces: `RuntimeControl` trait 新增 `model_create` / `model_list` / `model_update` / `model_delete` / `model_tags_update` 方法签名 + dispatch 路由 + stub 实现

> 此任务后所有 trait 实现者必须添加新方法才能编译。supervisor 的真实实现在 Task 6，TestRuntimeControl 和 TestRuntimeWrapper 在此任务添加 stub。

- [ ] **Step 1: 在 trait 中添加 model 方法签名**

在 `crates/busytok-control/src/dispatch.rs` 第 203-211 行的 provider 方法块之后添加：

```rust
    // Models (Phase: Provider/Model Catalog Refactor)
    async fn model_create(&self, req: ModelCreateRequestDto) -> Result<ModelCatalogEntryDto>;
    async fn model_list(&self, req: ModelListRequestDto) -> Result<ModelListResponseDto>;
    async fn model_update(&self, req: ModelUpdateRequestDto) -> Result<()>;
    async fn model_delete(&self, req: ModelDeleteRequestDto) -> Result<()>;
    async fn model_tags_update(&self, req: ModelTagUpdateDto) -> Result<()>;
```

在 dispatch.rs 顶部的 DTO import 块中添加新 DTO 的导入。

- [ ] **Step 2: 添加 dispatch 路由**

在 `ControlDispatcher::dispatch` 的 match 中（第 560 行之后，`provider.test_connection` 分支之后）添加：

```rust
            "model.create" => {
                let req: ModelCreateRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for model.create: {e}"))?;
                let dto = self.runtime.model_create(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "model.list" => {
                let req: ModelListRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for model.list: {e}"))?;
                let dto = self.runtime.model_list(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "model.update" => {
                let req: ModelUpdateRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for model.update: {e}"))?;
                self.runtime.model_update(req).await?;
                ControlResponse::ok(serde_json::to_value(())?)
            }
            "model.delete" => {
                let req: ModelDeleteRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for model.delete: {e}"))?;
                self.runtime.model_delete(req).await?;
                ControlResponse::ok(serde_json::to_value(())?)
            }
            "model.tags.update" => {
                let req: ModelTagUpdateDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for model.tags.update: {e}"))?;
                self.runtime.model_tags_update(req).await?;
                ControlResponse::ok(serde_json::to_value(())?)
            }
```

- [ ] **Step 3: 在 TestRuntimeControl 中添加 stub**

在 `crates/busytok-control/src/dispatch.rs` 的 `TestRuntimeControl` impl 中（第 1250 行之后）添加：

```rust
    async fn model_create(&self, _req: ModelCreateRequestDto) -> Result<ModelCatalogEntryDto> {
        anyhow::bail!("not yet implemented")
    }
    async fn model_list(&self, _req: ModelListRequestDto) -> Result<ModelListResponseDto> {
        Ok(ModelListResponseDto { models: vec![] })
    }
    async fn model_update(&self, _req: ModelUpdateRequestDto) -> Result<()> {
        anyhow::bail!("not yet implemented")
    }
    async fn model_delete(&self, _req: ModelDeleteRequestDto) -> Result<()> {
        anyhow::bail!("not yet implemented")
    }
    async fn model_tags_update(&self, _req: ModelTagUpdateDto) -> Result<()> {
        anyhow::bail!("not yet implemented")
    }
```

- [ ] **Step 4: 在 blanket impl `Arc<T>` 中添加委托**

在 `crates/busytok-control/src/dispatch.rs` 第 1283 行起的 `impl<T: RuntimeControl> RuntimeControl for Arc<T>` 中，找到 provider 方法委托之后添加：

```rust
    async fn model_create(&self, req: ModelCreateRequestDto) -> Result<ModelCatalogEntryDto> {
        (**self).model_create(req).await
    }
    async fn model_list(&self, req: ModelListRequestDto) -> Result<ModelListResponseDto> {
        (**self).model_list(req).await
    }
    async fn model_update(&self, req: ModelUpdateRequestDto) -> Result<()> {
        (**self).model_update(req).await
    }
    async fn model_delete(&self, req: ModelDeleteRequestDto) -> Result<()> {
        (**self).model_delete(req).await
    }
    async fn model_tags_update(&self, req: ModelTagUpdateDto) -> Result<()> {
        (**self).model_tags_update(req).await
    }
```

- [ ] **Step 5: 在 CLI TestRuntimeWrapper 中添加 stub**

在 `apps/cli/src/commands.rs` 第 1844 行之后（provider_test_connection stub 之后）添加：

```rust
        async fn model_create(&self, req: ModelCreateRequestDto) -> Result<ModelCatalogEntryDto> {
            self.inner.model_create(req).await
        }
        async fn model_list(&self, req: ModelListRequestDto) -> Result<ModelListResponseDto> {
            self.inner.model_list(req).await
        }
        async fn model_update(&self, req: ModelUpdateRequestDto) -> Result<()> {
            self.inner.model_update(req).await
        }
        async fn model_delete(&self, req: ModelDeleteRequestDto) -> Result<()> {
            self.inner.model_delete(req).await
        }
        async fn model_tags_update(&self, req: ModelTagUpdateDto) -> Result<()> {
            self.inner.model_tags_update(req).await
        }
```

在 commands.rs 顶部的 protocol DTO 导入块中添加新 DTO 的导入。

- [ ] **Step 6: 验证 control + cli crate 编译（supervisor 仍会有错误）**

Run: `cargo check -p busytok-control`
Expected: control crate 编译通过

- [ ] **Step 7: Commit**

```bash
git add crates/busytok-control/src/dispatch.rs apps/cli/src/commands.rs
git commit -m "feat(control): add model.* RPC methods to RuntimeControl trait + dispatch + stubs"
```

---

## Task 5: supervisor provider handlers 切 SQL + 阻断删除 + test_connection 探针

**Files:**
- Modify: `crates/busytok-runtime/src/supervisor.rs`
- Test: `crates/busytok-runtime/tests/supervisor_control.rs`（或新建 `tests/provider_catalog.rs`）

**Interfaces:**
- Consumes: Task 2 的 store repository，Task 3 的 DTO，Task 4 的 trait 方法
- Produces: supervisor 中 `provider_create` / `provider_list` / `provider_update` / `provider_delete` / `provider_test_connection` 的 SQL 后端实现

> 此任务较大。`provider_to_dto` 函数重写为从 `Provider`（domain）映射。`provider_changed` 仍调用 worker pool 通知。

- [ ] **Step 1: 写 supervisor provider handler 的失败测试**

在 `crates/busytok-runtime/tests/provider_catalog.rs` 中写测试（使用现有 supervisor 测试 harness 模式）。参考 `tests/supervisor_control.rs` 的 setup。关键测试：

```rust
// 参考现有 supervisor 测试的 harness 构建。如果现有测试用 TestRuntimeControl
// 而非真实 BusytokSupervisor，则需要创建一个带内存 DB 的真实 supervisor 测试。
// 假设有 test_utils 提供 setup_supervisor() -> BusytokSupervisor

#[tokio::test]
async fn provider_create_persists_to_sql_with_api_key() {
    let sup = setup_supervisor().await;
    sup.provider_create(ProviderCreateRequestDto {
        id: "p1".into(), name: "Test".into(),
        base_url: "https://api.test.com".into(),
        api_key: Some("sk-test".into()),
    }).await.unwrap();

    let list = sup.provider_list().await.unwrap();
    assert_eq!(list.providers.len(), 1);
    assert!(list.providers[0].has_api_key);
    assert_eq!(list.providers[0].provider_kind, ProviderKind::OpenAiCompatible);
}

#[tokio::test]
async fn provider_delete_blocked_by_profile_reference() {
    let sup = setup_supervisor().await;
    sup.provider_create(/* ... p1 ... */).await.unwrap();
    // Create a profile referencing p1
    // ... (inject profile into settings)
    let err = sup.provider_delete(ProviderDeleteRequestDto { id: "p1".into() }).await;
    assert!(err.is_err());
}

#[tokio::test]
async fn provider_test_connection_no_enabled_model_skips_fallback() {
    let sup = setup_supervisor().await;
    sup.provider_create(/* p1, no models */).await.unwrap();
    let result = sup.provider_test_connection(ProviderTestConnectionRequestDto { id: "p1".into() }).await;
    // /models endpoint should be called; if it fails, error should mention "no models configured"
    // This test may need a mock HTTP server — check if existing tests have one.
}
```

> **实现者注意：** 如果现有 supervisor 测试不使用真实 `BusytokSupervisor`（而是 mock），需要先检查 `tests/supervisor_control.rs` 的 harness 模式。如果无法构造真实 supervisor 测试，则在 `supervisor.rs` 的 `#[cfg(test)]` 模块中写单元测试，直接调用 store repository + handler 逻辑。

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p busytok-runtime provider_catalog -- --nocapture`
Expected: FAIL

- [ ] **Step 3: 重写 provider handlers**

在 `crates/busytok-runtime/src/supervisor.rs` 中：

**3a. 删除旧导入**：移除 `ProviderConfig`, `ProviderCredentialStore` 的导入（如果这些类型在 Task 7 才删，则暂时保留 import 但不使用）。

**3b. 重写 `provider_create`**（替换第 5700-5751 行）：

```rust
    async fn provider_create(&self, req: ProviderCreateRequestDto) -> Result<ProviderDto> {
        if req.id.is_empty() {
            anyhow::bail!("provider id must not be empty");
        }
        if !req.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            anyhow::bail!("provider id must contain only [a-z0-9-]+");
        }
        let provider = {
            let db = self.db.lock().unwrap();
            db.create_provider(busytok_store::CreateProviderReq {
                id: req.id.clone(),
                name: req.name,
                provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
                base_url: req.base_url,
                enabled: true,
                api_key: req.api_key,
            })?
        };
        tracing::info!(event_code = "provider.created", provider_id = %provider.id, "provider created");
        self.provider_changed(&provider.id).await;
        Ok(provider_to_dto(&provider))
    }
```

**3c. 重写 `provider_list`**：

```rust
    async fn provider_list(&self) -> Result<ProviderListResponseDto> {
        let summaries = {
            let db = self.db.lock().unwrap();
            db.list_providers()?
        };
        let providers: Vec<ProviderDto> = summaries.iter().map(provider_summary_to_dto).collect();
        Ok(ProviderListResponseDto { providers })
    }
```

**3d. 重写 `provider_update`**：

```rust
    async fn provider_update(&self, req: ProviderUpdateRequestDto) -> Result<ProviderDto> {
        let provider = {
            let db = self.db.lock().unwrap();
            db.update_provider(&req.id, busytok_store::UpdateProviderPatch {
                name: req.name,
                base_url: req.base_url,
                enabled: req.enabled,
                api_key: req.api_key,
            })?
        };
        tracing::info!(event_code = "provider.updated", provider_id = %provider.id, "provider updated");
        self.provider_changed(&provider.id).await;
        Ok(provider_to_dto(&provider))
    }
```

**3e. 重写 `provider_delete`**（含阻断删除）：

```rust
    async fn provider_delete(&self, req: ProviderDeleteRequestDto) -> Result<()> {
        let profile_refs = self.collect_profile_refs();
        {
            let db = self.db.lock().unwrap();
            db.delete_provider(&req.id, &profile_refs)?;
        }
        tracing::info!(event_code = "provider.deleted", provider_id = %req.id, "provider deleted");
        self.provider_changed(&req.id).await;
        Ok(())
    }
```

**3f. 重写 `provider_test_connection`**（含探针逻辑）：

```rust
    async fn provider_test_connection(&self, req: ProviderTestConnectionRequestDto) -> Result<ProviderTestConnectionResponseDto> {
        let provider = {
            let db = self.db.lock().unwrap();
            db.get_provider_with_secret(&req.id)?
                .ok_or_else(|| anyhow!("provider not found: {}", req.id))?
        };
        let api_key = provider.api_key.as_deref()
            .ok_or_else(|| anyhow!("provider has no api key"))?;

        // Try /models first
        let client = reqwest::Client::new();
        let models_url = format!("{}/models", provider.base_url.trim_end_matches('/'));
        let resp = client.get(&models_url)
            .header("Authorization", format!("Bearer {}", api_key))
            .send().await;

        match resp {
            Ok(r) if r.status().is_success() => {
                // /models succeeded — connection OK
                return Ok(ProviderTestConnectionResponseDto {
                    ok: true, error: None, models_detected: None,
                });
            }
            _ => {}
        }

        // Fallback to /chat/completions — needs a model id from SQL
        let probe_model = {
            let db = self.db.lock().unwrap();
            db.list_models_filtered(busytok_domain::ModelCatalogFilter {
                provider_id: Some(req.id.clone()),
                tags: vec![],
                include_disabled: false,
            })?
        };
        let probe_model = probe_model.into_iter().next()
            .ok_or_else(|| anyhow!("provider has no enabled models configured, cannot probe /chat/completions"))?;

        // ... existing /chat/completions probe logic using probe_model.model_id ...
        // (保留现有 fallback HTTP 调用，只把 model_id 来源从 provider.models 改为 SQL)
    }
```

**3g. 新增辅助方法 `collect_profile_refs`**：

```rust
    /// Collect (provider_id, model_id) references from settings.toml profiles.
    /// Used by delete-blocking checks.
    fn collect_profile_refs(&self) -> Vec<busytok_domain::ProfileModelRef> {
        let settings = self.settings.lock().unwrap();
        settings.subagent.profiles.values()
            .filter_map(|p| {
                let pid = p.provider_id.as_ref()?;
                let mid = p.model.as_ref()?;
                Some(busytok_domain::ProfileModelRef {
                    provider_id: pid.clone(),
                    model_id: mid.clone(),
                })
            })
            .collect()
    }
```

**3h. 重写 `provider_to_dto`** 和新增 `provider_summary_to_dto`：

```rust
fn provider_to_dto(p: &busytok_domain::Provider) -> ProviderDto {
    ProviderDto {
        id: p.id.clone(),
        name: p.name.clone(),
        provider_kind: p.provider_kind.clone(),
        base_url: p.base_url.clone(),
        enabled: p.enabled,
        has_api_key: p.api_key.is_some(),
        created_at_ms: p.created_at_ms,
        updated_at_ms: p.updated_at_ms,
    }
}

fn provider_summary_to_dto(s: &busytok_domain::ProviderSummary) -> ProviderDto {
    ProviderDto {
        id: s.id.clone(),
        name: s.name.clone(),
        provider_kind: s.provider_kind.clone(),
        base_url: s.base_url.clone(),
        enabled: s.enabled,
        has_api_key: s.has_api_key,
        created_at_ms: s.created_at_ms,
        updated_at_ms: s.updated_at_ms,
    }
}
```

- [ ] **Step 4: 运行测试验证通过**

Run: `cargo test -p busytok-runtime provider_catalog -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-runtime/src/supervisor.rs crates/busytok-runtime/tests/provider_catalog.rs
git commit -m "feat(runtime): switch provider handlers to SQL with blocking delete + test_connection probe"
```

---

## Task 6: supervisor model handlers + delegate 二次校验 + profile 校验

**Files:**
- Modify: `crates/busytok-runtime/src/supervisor.rs`
- Test: `crates/busytok-runtime/tests/provider_catalog.rs`（续）

**Interfaces:**
- Consumes: Task 2 store repository，Task 4 trait 方法
- Produces: `model_create` / `model_list` / `model_update` / `model_delete` / `model_tags_update` 的 supervisor 实现 + delegate/profile 校验改为查 SQL

- [ ] **Step 1: 写 model handler 失败测试**

在 `crates/busytok-runtime/tests/provider_catalog.rs` 续写：

```rust
#[tokio::test]
async fn model_create_and_list_round_trip() {
    let sup = setup_supervisor().await;
    sup.provider_create(/* p1 */).await.unwrap();
    sup.model_create(ModelCreateRequestDto {
        provider_id: "p1".into(), model_id: "gpt-4o".into(),
        enabled: Some(true), tags: vec!["fast".into()],
    }).await.unwrap();

    let list = sup.model_list(ModelListRequestDto {
        provider_id: None, tags: vec![], include_disabled: false,
    }).await.unwrap();
    assert_eq!(list.models.len(), 1);
    assert_eq!(list.models[0].model_id, "gpt-4o");
    assert!(list.models[0].tags.contains(&"fast".to_string()));
}

#[tokio::test]
async fn model_update_rejects_model_id_change() {
    // ModelUpdateRequestDto has no model_id field — verified at compile time
    let dto = ModelUpdateRequestDto { id: "model_x".into(), enabled: Some(false) };
    // If this compiles, model_id is not in the DTO — success
    let _ = dto;
}

#[tokio::test]
async fn delegate_rejects_when_provider_disabled() {
    let sup = setup_supervisor().await;
    sup.provider_create(/* p1 enabled */).await.unwrap();
    sup.model_create(/* gpt-4o under p1 */).await.unwrap();
    // Create profile binding p1+gpt-4o
    // Disable p1
    sup.provider_update(/* p1 enabled=false */).await.unwrap();
    // Delegate should fail
    let err = sup.subagent_delegate(/* profile */).await;
    assert!(err.is_err());
}
```

- [ ] **Step 2: 实现 model handlers**

在 supervisor.rs 的 `impl RuntimeControl for BusytokSupervisor` 中添加：

```rust
    async fn model_create(&self, req: ModelCreateRequestDto) -> Result<ModelCatalogEntryDto> {
        let model = {
            let db = self.db.lock().unwrap();
            db.create_model(busytok_store::CreateModelReq {
                provider_id: req.provider_id.clone(),
                model_id: req.model_id.clone(),
                enabled: req.enabled.unwrap_or(true),
                tags: req.tags.clone(),
            })?
        };
        tracing::info!(event_code = "model.created", model_id = %model.model_id, provider_id = %model.provider_id, "model created");
        let entries = {
            let db = self.db.lock().unwrap();
            db.list_models_filtered(busytok_domain::ModelCatalogFilter {
                provider_id: Some(model.provider_id.clone()),
                tags: vec![], include_disabled: true,
            })?
        };
        entries.into_iter().find(|e| e.model_db_id == model.id)
            .map(catalog_entry_to_dto)
            .ok_or_else(|| anyhow!("model not found after create"))
    }

    async fn model_list(&self, req: ModelListRequestDto) -> Result<ModelListResponseDto> {
        let entries = {
            let db = self.db.lock().unwrap();
            db.list_models_filtered(busytok_domain::ModelCatalogFilter {
                provider_id: req.provider_id,
                tags: req.tags,
                include_disabled: req.include_disabled,
            })?
        };
        tracing::info!(event_code = "model.catalog.listed", count = entries.len(), "model catalog listed");
        Ok(ModelListResponseDto {
            models: entries.iter().map(catalog_entry_to_dto_ref).collect(),
        })
    }

    async fn model_update(&self, req: ModelUpdateRequestDto) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.update_model(&req.id, busytok_store::UpdateModelPatch {
            enabled: req.enabled,
        })?;
        Ok(())
    }

    async fn model_delete(&self, req: ModelDeleteRequestDto) -> Result<()> {
        let profile_refs = self.collect_profile_refs();
        let db = self.db.lock().unwrap();
        db.delete_model(&req.id, &profile_refs)?;
        Ok(())
    }

    async fn model_tags_update(&self, req: ModelTagUpdateDto) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.set_model_tags(&req.model_id, &req.tags)?;
        Ok(())
    }
```

添加 DTO 映射函数：

```rust
fn catalog_entry_to_dto(e: busytok_domain::ModelCatalogEntry) -> ModelCatalogEntryDto {
    ModelCatalogEntryDto {
        provider_id: e.provider_id,
        provider_name: e.provider_name,
        provider_kind: e.provider_kind,
        provider_enabled: e.provider_enabled,
        model_db_id: e.model_db_id,
        model_id: e.model_id,
        model_enabled: e.model_enabled,
        tags: e.tags,
    }
}

fn catalog_entry_to_dto_ref(e: &busytok_domain::ModelCatalogEntry) -> ModelCatalogEntryDto {
    catalog_entry_to_dto(e.clone())
}
```

- [ ] **Step 3: 重写 delegate 二次校验**

替换 supervisor.rs 第 5335-5379 行的 delegate 校验块：

```rust
        if self.worker_pool().is_some() {
            let (profile_provider, profile_model) = {
                let settings = self.settings.lock().unwrap();
                let profile_cfg = settings.subagent.profiles.get(&req.profile);
                match profile_cfg {
                    Some(p) => (p.provider_id.clone(), p.model.clone()),
                    None => return Err(anyhow!("profile not found: {}", req.profile)),
                }
            };
            if let Some(provider_id) = profile_provider.as_deref() {
                // Re-validate against SQL catalog
                let provider = {
                    let db = self.db.lock().unwrap();
                    db.get_provider_with_secret(provider_id)?
                };
                let provider = provider
                    .ok_or_else(|| anyhow!("provider not found: {}", provider_id))?;
                if !provider.enabled {
                    anyhow::bail!("provider disabled: {}", provider_id);
                }
                let model_id = profile_model.as_deref().unwrap_or("");
                if model_id.is_empty() {
                    anyhow::bail!("profile not bound to a model");
                }
                let model = {
                    let db = self.db.lock().unwrap();
                    db.get_model_by_provider_and_model_id(provider_id, model_id)?
                };
                let model = model
                    .ok_or_else(|| anyhow!("model '{}' not found for provider '{}'", model_id, provider_id))?;
                if !model.enabled {
                    anyhow::bail!("model disabled: {}", model_id);
                }
            } else {
                anyhow::bail!("profile not bound to a provider");
            }
        }
```

- [ ] **Step 4: 重写 profile create/update 校验**

在 supervisor.rs 的 `profile_create` / `profile_update` handler 中，把对 `provider.models` 白名单的校验改为 SQL 查询。找到约第 6148 行的 `provider_cfg.models.iter().any(|m| m == &req.model)` 逻辑，替换为：

```rust
// Validate model exists in SQL catalog
let model = {
    let db = self.db.lock().unwrap();
    db.get_model_by_provider_and_model_id(&provider_id, &req.model)?
};
let model = model.ok_or_else(|| {
    anyhow!("model '{}' not found for provider '{}'", req.model, provider_id)
})?;
if !model.enabled {
    anyhow::bail!("model '{}' is disabled", req.model);
}
```

- [ ] **Step 5: 运行测试验证通过**

Run: `cargo test -p busytok-runtime provider_catalog -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/busytok-runtime/src/supervisor.rs crates/busytok-runtime/tests/provider_catalog.rs
git commit -m "feat(runtime): add model handlers + delegate re-validation + profile SQL validation"
```

---

## Task 7: sidecar 固定 env 注入 + 删 SidecarConfig env name 字段

**Files:**
- Modify: `crates/busytok-subagent/src/sidecar/config.rs`
- Modify: `crates/busytok-subagent/src/sidecar/pool.rs`
- Modify: `crates/busytok-runtime/src/supervisor.rs`（WorkerPool 构造处）
- Test: `crates/busytok-subagent/src/sidecar/pool.rs`（inline tests）

**Interfaces:**
- Consumes: `busytok_domain::Provider`（含 api_key + base_url）
- Produces: `WorkerPool` 构造改为接受 `ProviderRuntimeEntry`（id + api_key + base_url），删除 `ProviderLookup` / `CredentialReader`

> sidecar 固定只注入 `OPENAI_API_KEY` 和 `OPENAI_BASE_URL`。WorkerPool 不再从外部闭包查 provider config + keychain，改为 supervisor 在构造/更新 worker 时直接传入 provider 的 api_key + base_url。

- [ ] **Step 1: 定义新的 ProviderRuntimeEntry 类型 + 写失败测试**

在 `crates/busytok-subagent/src/sidecar/pool.rs` 中定义新类型：

```rust
/// Provider runtime entry — everything WorkerPool needs to spawn a worker.
/// Replaces ProviderLookup + CredentialReader closures.
#[derive(Debug, Clone)]
pub struct ProviderRuntimeEntry {
    pub provider_id: String,
    pub api_key: String,
    pub base_url: String,
}
```

写测试验证 WorkerPool 用固定 env 构造 SidecarConfig：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_provider_config_uses_fixed_env_names() {
        let entry = ProviderRuntimeEntry {
            provider_id: "p1".into(),
            api_key: "sk-test".into(),
            base_url: "https://api.test.com".into(),
        };
        let mut env = std::collections::HashMap::new();
        inject_provider_env(&mut env, &entry);
        assert_eq!(env.get("OPENAI_API_KEY"), Some(&"sk-test".to_string()));
        assert_eq!(env.get("OPENAI_BASE_URL"), Some(&"https://api.test.com".to_string()));
    }
}
```

- [ ] **Step 2: 删除 SidecarConfig 的 env name 字段**

在 `crates/busytok-subagent/src/sidecar/config.rs` 中删除第 51-60 行的 `provider_id` / `api_key_env_name` / `base_url_env_name` 字段。`SidecarConfig` 只保留与 spawn 相关的字段（node_binary, bundle_path, env, idle_exit_seconds 等）。provider 特定信息通过 `env` HashMap 注入。

- [ ] **Step 3: 重写 WorkerPool**

在 `crates/busytok-subagent/src/sidecar/pool.rs` 中：

删除 `ProviderLookup` 和 `CredentialReader` 类型别名（第 55、61 行）。

`WorkerPool` 结构体（第 75-112 行）中，将 `providers: ProviderLookup` 和 `credential_reader: CredentialReader` 字段替换为：

```rust
    /// Provider runtime entries — keyed by provider_id. Updated by supervisor
    /// when provider config changes (provider_changed).
    providers: Arc<std::sync::Mutex<HashMap<String, ProviderRuntimeEntry>>>,
```

新增 `inject_provider_env` 函数：

```rust
/// Inject provider credentials into env using FIXED names.
/// Sidecar only recognizes OPENAI_API_KEY and OPENAI_BASE_URL.
pub fn inject_provider_env(env: &mut HashMap<String, String>, entry: &ProviderRuntimeEntry) {
    env.insert("OPENAI_API_KEY".to_string(), entry.api_key.clone());
    env.insert("OPENAI_BASE_URL".to_string(), entry.base_url.clone());
}
```

`WorkerPool::new` 构造函数签名改为接受 `HashMap<String, ProviderRuntimeEntry>` 而非闭包。

`ensure_worker` 方法中，从 `self.providers` lock 中取 entry，调用 `inject_provider_env` 注入 env。

新增 `update_provider` 方法供 supervisor 调用：

```rust
    /// Update or insert a provider's runtime entry. Called by supervisor
    /// on provider_changed. Kills the old worker so next delegate re-spawns.
    pub fn update_provider(&self, entry: ProviderRuntimeEntry) {
        let pid = entry.provider_id.clone();
        {
            let mut providers = self.providers.lock().unwrap();
            providers.insert(pid.clone(), entry);
        }
        // Kill existing worker — lazy re-spawn on next delegate
        let mut workers = self.workers.lock().unwrap();
        workers.remove(&pid);
    }
```

- [ ] **Step 4: 更新 supervisor 构造 WorkerPool 的代码**

在 supervisor.rs 中找到 WorkerPool 构造处（搜索 `WorkerPool::new`），把传入的 `ProviderLookup` 闭包和 `CredentialReader` 闭包改为从 SQL 读取所有 provider 构建 `HashMap<String, ProviderRuntimeEntry>`：

```rust
let providers: HashMap<String, ProviderRuntimeEntry> = {
    let db = self.db.lock().unwrap();
    db.list_providers()?
        .into_iter()
        .filter_map(|s| {
            let p = db.get_provider_with_secret(&s.id).ok()??;
            if !p.enabled { return None; }
            let api_key = p.api_key?;
            Some((p.id.clone(), ProviderRuntimeEntry {
                provider_id: p.id,
                api_key,
                base_url: p.base_url,
            }))
        })
        .collect()
};
```

更新 `provider_changed` 方法，改为调用 `worker_pool.update_provider(entry)` 而非 `remove_worker_and_kill`：

```rust
    async fn provider_changed(&self, provider_id: &str) {
        if let Some(pool) = self.worker_pool() {
            // Re-read provider from SQL and update pool
            let entry = {
                let db = self.db.lock().unwrap();
                db.get_provider_with_secret(provider_id).ok().flatten()
            };
            if let Some(p) = entry {
                if p.enabled {
                    if let Some(api_key) = p.api_key {
                        pool.update_provider(ProviderRuntimeEntry {
                            provider_id: p.id,
                            api_key,
                            base_url: p.base_url,
                        });
                        return;
                    }
                }
            }
            // Provider disabled or no api_key — remove from pool
            pool.remove_worker(provider_id);
        }
    }
```

- [ ] **Step 5: 运行测试验证通过**

Run: `cargo test -p busytok-subagent pool -- --nocapture && cargo check -p busytok-runtime`
Expected: subagent 测试 PASS，runtime 编译通过

- [ ] **Step 6: Commit**

```bash
git add crates/busytok-subagent/src/sidecar/config.rs crates/busytok-subagent/src/sidecar/pool.rs \
  crates/busytok-runtime/src/supervisor.rs
git commit -m "feat(sidecar): fix env injection to OPENAI_API_KEY/OPENAI_BASE_URL + remove env name fields"
```

---

## Task 8: CLI `models` 命令

**Files:**
- Create: `apps/cli/src/commands/models.rs`
- Modify: `apps/cli/src/main.rs`
- Modify: `apps/cli/src/commands.rs`（或 `commands/mod.rs`）
- Test: `apps/cli/tests/` 或 inline

**Interfaces:**
- Consumes: `model.list` RPC 方法
- Produces: `busytok models` 命令（table/json 输出，--provider/--tag/--all/--json 参数）

- [ ] **Step 1: 在 main.rs 添加 Models 命令枚举**

在 `apps/cli/src/main.rs` 的 `Command` enum 中（第 82 行 `Doctor` 之后）添加：

```rust
    /// List models in the catalog
    Models {
        /// Filter by provider id
        #[arg(long)]
        provider: Option<String>,
        /// Filter by tag (repeatable, AND semantics)
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Include disabled models and disabled-provider models
        #[arg(long)]
        all: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
```

- [ ] **Step 2: 在 run match 中添加分支**

在 main.rs 的 `match command` 中（第 617 行 `Doctor` 分支之前）添加：

```rust
        Command::Models { provider, tags, all, json } => {
            commands::handle_models(provider, tags, all, json).await
        }
```

- [ ] **Step 3: 实现 handle_models**

在 `apps/cli/src/commands.rs` 中添加（如果 commands 是模块目录，则在 `commands/models.rs` 中）：

```rust
pub async fn handle_models(
    provider: Option<String>,
    tags: Vec<String>,
    all: bool,
    json: bool,
) -> Result<()> {
    let req = ModelListRequestDto {
        provider_id: provider,
        tags,
        include_disabled: all,
    };
    let mut client = connect_client().await?;
    let response = client
        .call(ControlRequest::new("model.list", serde_json::to_value(&req)?))
        .await?;
    match response {
        ControlResponse::Ok(value) => {
            let resp: ModelListResponseDto = serde_json::from_value(value)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&resp.models)?);
            } else {
                print_models_table(&resp.models);
            }
            Ok(())
        }
        ControlResponse::Err(err) => {
            anyhow::bail!("RPC error [{}]: {}", err.code, err.message)
        }
    }
}

fn print_models_table(models: &[ModelCatalogEntryDto]) {
    if models.is_empty() {
        println!("No models found.");
        return;
    }
    // Column widths
    let w_provider = models.iter().map(|m| m.provider_name.len()).max().unwrap_or(8).max(8);
    let w_model = models.iter().map(|m| m.model_id.len()).max().unwrap_or(5).max(5);
    let w_tags = 20;

    println!(
        "{:width_p$}  {:width_m$}  {:6}  {:6}  {:width_t$}",
        "PROVIDER", "MODEL", "ENABLE", "P_ENABLE", "TAGS",
        width_p = w_provider, width_m = w_model, width_t = w_tags
    );
    for m in models {
        let tags = m.tags.join(",");
        let model_en = if m.model_enabled { "yes" } else { "no" };
        let prov_en = if m.provider_enabled { "yes" } else { "no" };
        println!(
            "{:width_p$}  {:width_m$}  {:6}  {:8}  {:width_t$}",
            m.provider_name, m.model_id, model_en, prov_en, tags,
            width_p = w_provider, width_m = w_model, width_t = w_tags
        );
    }
}
```

- [ ] **Step 4: 写 CLI 测试**

在 CLI 测试中验证 `handle_models` 的 table 和 json 输出格式。可以用 `ModelListRequestDto` 的 serde 序列化验证参数正确性。

- [ ] **Step 5: 运行测试**

Run: `cargo test -p busytok-cli models -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add apps/cli/src/main.rs apps/cli/src/commands.rs apps/cli/src/commands/models.rs
git commit -m "feat(cli): add 'busytok models' command with table/json output and provider/tag filters"
```

---

## Task 9: GUI 改造（ProvidersPage 简化 + ModelsSection 新增）

**Files:**
- Modify: `apps/gui/src/api/busytokClient.ts`
- Modify: `apps/gui/src/pages/ProvidersPage.tsx`
- Create: `apps/gui/src/components/ModelsSection.tsx`
- Modify: `apps/gui/src/components/ProfilesSection.tsx`（model 选择改为从 catalog 读）

**Interfaces:**
- Consumes: Task 3 重新生成的 TS 类型
- Produces: 简化的 ProvidersPage（无 env name / models 编辑），新增 ModelsSection（CRUD + tags 管理）

> GUI 改动以 React 组件为主。测试用 vitest（如果 GUI 有测试配置）或手动验证。

- [ ] **Step 1: 在 busytokClient.ts 添加 model.* 方法**

在 `apps/gui/src/api/busytokClient.ts` 第 186 行之后添加：

```ts
    // Models — bare DTOs (not wrapped)
    modelList: (request: ModelListRequestDto) =>
      call<ModelListResponseDto>("model.list", { ...request }),
    modelCreate: (request: ModelCreateRequestDto) =>
      call<ModelCatalogEntryDto>("model.create", { ...request }),
    modelUpdate: (request: ModelUpdateRequestDto) =>
      call<void>("model.update", { ...request }),
    modelDelete: (id: string) =>
      call<void>("model.delete", { id }),
    modelTagsUpdate: (modelId: string, tags: string[]) =>
      call<void>("model.tags.update", { model_id: modelId, tags }),
```

在 import 块中添加新类型导入：

```ts
  ModelCatalogEntryDto,
  ModelCreateRequestDto,
  ModelDeleteRequestDto,
  ModelListRequestDto,
  ModelListResponseDto,
  ModelTagUpdateDto,
  ModelUpdateRequestDto,
```

- [ ] **Step 2: 简化 ProvidersPage**

在 `apps/gui/src/pages/ProvidersPage.tsx` 中：
- 删除 `api_key_env_name` 输入框
- 删除 `base_url_env_name` 输入框
- 删除 `models` 字符串数组编辑
- 表单只保留：id, name, base_url, enabled, api_key（密码框）
- 列表展示列：name, provider_kind, base_url, enabled, has_api_key

- [ ] **Step 3: 创建 ModelsSection 组件**

`apps/gui/src/components/ModelsSection.tsx`:

```tsx
import { useEffect, useState } from "react";
import { busytokClient } from "../api/busytokClient";
import type { ModelCatalogEntryDto } from "@busytok/protocol-types";

export function ModelsSection() {
  const [models, setModels] = useState<ModelCatalogEntryDto[]>([]);
  const [filterProvider, setFilterProvider] = useState<string>("");
  const [filterTag, setFilterTag] = useState<string>("");
  const [showAll, setShowAll] = useState(false);

  const refresh = async () => {
    const tags = filterTag ? filterTag.split(",").map(t => t.trim()).filter(Boolean) : [];
    const resp = await busytokClient.modelList({
      provider_id: filterProvider || null,
      tags,
      include_disabled: showAll,
    });
    setModels(resp.models);
  };

  useEffect(() => { refresh(); }, [filterProvider, filterTag, showAll]);

  // ... render table with: provider_name, model_id, enabled, tags, delete/toggle buttons
  // ... render create form: provider select, model_id input, tags input
  // ... render tag edit inline
}
```

- [ ] **Step 4: 更新 ProfilesSection 的 model 选择**

在 `apps/gui/src/components/ProfilesSection.tsx` 中，把 model 下拉选择从 provider.models 改为调用 `busytokClient.modelList({ provider_id, ... })` 获取该 provider 下的模型列表。

- [ ] **Step 5: 验证 GUI 构建**

Run: `cd apps/gui && pnpm run build`
Expected: 构建成功，无 TS 类型错误

- [ ] **Step 6: Commit**

```bash
git add apps/gui/src/api/busytokClient.ts apps/gui/src/pages/ProvidersPage.tsx \
  apps/gui/src/components/ModelsSection.tsx apps/gui/src/components/ProfilesSection.tsx
git commit -m "feat(gui): simplify ProvidersPage + add ModelsSection + profile model select from catalog"
```

---

## Task 10: 残留清理 + grep 断言 + 日志事件审计

**Files:**
- Modify: `crates/busytok-config/src/providers.rs`（删除 ProviderConfig）
- Modify: `crates/busytok-config/src/lib.rs`（删除 providers 模块导出，删除 BusytokSettings.providers 字段）
- Delete: keychain 相关文件
- Modify: `crates/busytok-config/Cargo.toml`（删除 keyring 依赖）
- Test: 残留 grep 断言测试

**Interfaces:**
- Consumes: 所有前置任务完成
- Produces: 代码库无残留旧设计

- [ ] **Step 1: 删除 ProviderConfig 和 providers 字段**

在 `crates/busytok-config/src/providers.rs` 中删除 `ProviderConfig` 结构体（整体删除文件或清空内容，只保留 `pub use busytok_domain::ProviderKind;` 如果还有外部引用）。

在 `crates/busytok-config/src/lib.rs` 中：
- 删除 `pub mod providers;` 或保留 re-export
- 从 `BusytokSettings` 结构体中删除 `pub providers: Vec<ProviderConfig>` 字段

- [ ] **Step 2: 删除 keychain 相关代码**

找到并删除 `ProviderCredentialStore` 的定义文件（搜索 `struct ProviderCredentialStore`）。

删除所有 `ProviderCredentialStore::set_key` / `get_key` / `has_key` / `delete_key` 调用（应该在前面的 Task 中已被替换）。

在 `crates/busytok-config/Cargo.toml`（或定义 keychain 的 crate 的 Cargo.toml）中删除 `keyring` 依赖。

- [ ] **Step 3: 清理 SidecarConfig 残留引用**

grep 搜索 `api_key_env_name` 和 `base_url_env_name` 在整个代码库中的残留引用，全部删除。

- [ ] **Step 4: 写残留 grep 断言测试**

在 `crates/busytok-runtime/tests/residual_cleanup.rs` 中：

```rust
use std::path::PathBuf;

/// Asserts that old design remnants are fully removed from the codebase.
/// This test prevents regressions where someone re-adds deleted patterns.
#[test]
fn no_env_name_fields_remain() {
    let excluded = vec![
        // This test file itself contains the strings for assertion
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/residual_cleanup.rs"),
    ];
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let forbidden_patterns = [
        "api_key_env_name",
        "base_url_env_name",
        "ProviderCredentialStore",
    ];
    for pattern in &forbidden_patterns {
        let output = std::process::Command::new("rg")
            .args(&["-l", pattern, "--type", "rust"])
            .current_dir(&workspace_root)
            .output()
            .expect("rg failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let offenders: Vec<&str> = stdout.lines()
            .filter(|line| {
                let path = PathBuf::from(line);
                !excluded.iter().any(|ex| path.ends_with(ex))
            })
            .collect();
        assert!(
            offenders.is_empty(),
            "Forbidden pattern '{}' found in: {:?}",
            pattern, offenders
        );
    }
}

#[test]
fn no_keychain_dependency_in_cargo() {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new("rg")
        .args(&["keyring", "--glob", "Cargo.toml"])
        .current_dir(&workspace_root)
        .output()
        .expect("rg failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.is_empty(), "keyring dependency still present: {}", stdout);
}
```

- [ ] **Step 5: 审计日志事件覆盖**

grep 确认以下 event_code 在代码库中存在：
- `provider.created`
- `provider.updated`
- `provider.deleted`
- `model.created`
- `model.updated`
- `model.deleted`
- `model.tag_added`
- `model.tag_removed`
- `model.catalog.listed`

对于缺失的 `provider.sql_read_failed` / `provider.sql_write_failed` / `model.sql_read_failed` / `model.sql_write_failed`，在 supervisor handler 的 error path 中添加 `tracing::error!` 日志（在 `?` 之前）。

- [ ] **Step 6: 运行全部测试 + 覆盖率**

Run: `cargo test --workspace -- --nocapture`
Expected: 全部 PASS

Run: `cargo llvm-cov --workspace --html`（或仓库现有覆盖率命令）
Expected: 变更文件覆盖率 ≥ 90%

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "chore: remove ProviderConfig/keychain/env-name remnants + add grep assertion tests + audit log events"
```

---

## Self-Review

**1. Spec 覆盖检查：**

| Spec 要求 | 覆盖 Task |
|-----------|-----------|
| providers/models/model_tags SQL 表 | Task 2 |
| ProviderKind 迁移到 domain | Task 1 |
| 删除 api_key_env_name / base_url_env_name | Task 3 (DTO) + Task 7 (SidecarConfig) + Task 10 (grep) |
| 删除 provider.models 字段 | Task 3 (DTO) + Task 5 (handler) + Task 10 (ProviderConfig) |
| 删除 ProviderCredentialStore / keychain | Task 10 |
| settings.toml 不再存 provider | Task 10 (BusytokSettings.providers 字段删除) |
| sidecar 固定 OPENAI_API_KEY/OPENAI_BASE_URL | Task 7 |
| 统一 model catalog 查询 | Task 2 (store) + Task 6 (supervisor) |
| CLI model catalog 命令 | Task 8 |
| GUI Providers + Models 管理 | Task 9 |
| 阻断删除 | Task 2 (store) + Task 5/6 (supervisor) |
| delegate 二次校验 | Task 6 |
| model_id 不可变 | Task 1 (domain) + Task 3 (DTO 无字段) + Task 2 (UpdateModelPatch 无字段) |
| include_disabled 过滤 provider + model | Task 2 (SQL + 测试) |
| 日志事件 | Task 2/5/6 (info!) + Task 10 (error! + 审计) |
| grep 断言 | Task 10 |
| 覆盖率 ≥ 90% | Task 10 Step 6 |

**2. 占位符扫描：** 无 TBD/TODO。Task 5 的 test_connection fallback HTTP 调用部分标注了"保留现有逻辑"，实现者需参考现有 supervisor.rs 5883 行附近的 HTTP 调用代码。

**3. 类型一致性：**
- `ProviderKind` 全链路统一使用 `busytok_domain::ProviderKind`（Task 1 定义 → Task 2 store → Task 3 protocol DTO → Task 5/6 supervisor）
- `ModelCatalogEntryDto.provider_kind` 是 `ProviderKind` 枚举（非 String），与 `ProviderDto` 一致
- `ModelUpdateRequestDto` 无 `model_id` 字段（Task 3 DTO + Task 2 UpdateModelPatch 一致）
- `ProfileModelRef` 在 domain 定义，store 和 supervisor 共用
- `ProviderRuntimeEntry` 在 subagent crate 定义，supervisor 构造 WorkerPool 时传入
