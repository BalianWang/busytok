# Provider / Model Catalog 存储架构重构

**日期**: 2026-07-03
**状态**: 已确认，待生成实施计划

## 1. 背景与目标

将 provider 存储从"provider 内嵌模型列表 + Keychain 密钥 + TOML 元数据"的混合方案，重构为统一的 SQL 领域模型。

**核心问题**：
- provider 元数据在 `settings.toml`，API key 在 Keychain，模型列表在 `provider.models: string[]` — 三源分裂
- `api_key_env_name` / `base_url_env_name` 把运行时 env 注入细节泄漏到配置层
- 模型不是一等实体，无法支持 tag、模型级查询、模型级治理

**目标**：
- `Provider` 只表示连接配置与认证信息
- `Model` 成为一等实体，按 `provider + model_id` 粒度管理
- `Tag` 成为模型的路由属性，多对多
- API key 统一存 SQL，不再用 Keychain
- CLI / GUI / Runtime 消费同一套 model catalog 读模型
- 不考虑旧数据兼容与迁移，直接按最终架构落地

## 2. 现状

| 维度 | 当前状态 |
|------|----------|
| Provider 领域模型 | `ProviderConfig` 在 `crates/busytok-config/src/providers.rs:19-37`，含 `models: Vec<String>` / `api_key_env_name` / `base_url_env_name`，序列化到 `settings.toml` 的 `[[providers]]` |
| Keychain | `ProviderCredentialStore`（`providers.rs:46-111`）用 `keyring` crate，集中在 `busytok-config` 一个文件 |
| SQL store | `rusqlite`（同步），schema version v5，30+ 张分析表，**无** providers/models/model_tags 表 |
| CLI | **完全没有** provider/model 命令 |
| GUI | `ProvidersPage.tsx`（723行）做 provider CRUD 含 `api_key_env_name` / `models` 输入框；`ProfilesSection.tsx` 读 `provider.models` 做下拉 |
| Sidecar env | `pool.rs:237-270` 动态构造 env name（`api_key_env_name` / `base_url_env_name`），同时冗余注入 `OPENAI_API_KEY` / `OPENAI_BASE_URL` |
| 日志 | `tracing` + `event_code` 字段，已有 `provider.created` / `provider.updated` / `provider.deleted` 事件 |

## 3. 关键决策（用户确认）

1. **Schema 策略**：追加 v6 迁移（`0006_provider_catalog.sql`），保留现有 30+ 张分析表及其数据。不写 provider 数据迁移逻辑（provider 数据原本在 TOML 不在 SQL）。
2. **ID 生成**：`providers.id` 和 `models.id` 全部用 UUID v4 自动生成。用户创建 provider 时不再输入 id。
3. **GUI 模型管理**：在 `ProvidersPage` 内嵌 Models 区块（不新增独立导航页）。
4. **实施路径**：Inside-out 分阶段（P1 domain+store → P2 翻转 → P3 CLI+GUI → P4 日志+测试+清理）。

## 4. Domain Model

放在 `crates/busytok-domain/src/provider_catalog.rs`。`ProviderKind` 从 `busytok-config` 迁移到 `busytok-domain`。

```rust
/// Provider = 连接能力。不含模型列表，不含 env name。
pub struct Provider {
    pub id: String,                  // UUID v4, 系统生成
    pub name: String,
    pub provider_kind: ProviderKind, // 当前仅 OpenAiCompatible
    pub base_url: String,
    pub enabled: bool,
    pub api_key: Option<String>,     // 写入时存原值；列表查询时返回 None
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Model = 可路由的模型实例。身份由 (provider_id, model_id) 定义。
pub struct Model {
    pub id: String,                  // UUID v4, 系统生成
    pub provider_id: String,         // FK → providers.id
    pub model_id: String,            // 如 "gpt-4o", "deepseek-chat"
    pub enabled: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Tag = 模型的路由属性，多对多。
pub struct ModelTag {
    pub model_id: String,            // FK → models.id
    pub tag: String,                 // 如 "fast", "cheap", "reasoning"
}

/// 统一模型目录视图（CLI/GUI/路由层共用读模型）
pub struct ModelCatalogEntry {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_kind: ProviderKind,
    pub provider_enabled: bool,
    pub model_db_id: String,         // models.id
    pub model_id: String,            // models.model_id
    pub model_enabled: bool,
    pub tags: Vec<String>,
}
```

## 5. SQL Schema

文件：`crates/busytok-store/migrations/0006_provider_catalog.sql`
`SCHEMA_VERSION` bump 5 → 6。

```sql
-- v6: Provider / Model / Tag catalog

CREATE TABLE providers (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    provider_kind TEXT NOT NULL DEFAULT 'openai_compatible',
    base_url      TEXT NOT NULL,
    enabled       INTEGER NOT NULL DEFAULT 1,
    api_key       TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE models (
    id            TEXT PRIMARY KEY,
    provider_id   TEXT NOT NULL,
    model_id      TEXT NOT NULL,
    enabled       INTEGER NOT NULL DEFAULT 1,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE,
    UNIQUE (provider_id, model_id)
);

CREATE TABLE model_tags (
    model_id TEXT NOT NULL,
    tag      TEXT NOT NULL,
    FOREIGN KEY (model_id) REFERENCES models(id) ON DELETE CASCADE,
    UNIQUE (model_id, tag)
);

CREATE INDEX idx_models_provider_id ON models(provider_id);
CREATE INDEX idx_model_tags_tag ON model_tags(tag);
```

- `api_key` 存为 `TEXT`（明文）。日志/DTO 严禁回显。
- `enabled` 用 `INTEGER`（0/1），SQLite 惯例。
- `ON DELETE CASCADE` 确保级联删除。
- 两个索引：按 provider 查模型、按 tag 反查模型。

## 6. Store Repository

文件：`crates/busytok-store/src/provider_catalog.rs`

### 写入接口

```rust
pub fn create_provider(conn: &Connection, req: CreateProviderReq) -> Result<Provider>;
pub fn update_provider(conn: &Connection, id: &str, patch: UpdateProviderPatch) -> Result<Provider>;
pub fn update_provider_api_key(conn: &Connection, id: &str, api_key: Option<&str>) -> Result<()>;
pub fn delete_provider(conn: &Connection, id: &str) -> Result<()>;

pub fn create_model(conn: &Connection, req: CreateModelReq) -> Result<Model>;
pub fn update_model(conn: &Connection, id: &str, patch: UpdateModelPatch) -> Result<Model>;
pub fn delete_model(conn: &Connection, id: &str) -> Result<()>;

pub fn add_model_tag(conn: &Connection, model_id: &str, tag: &str) -> Result<()>;
pub fn remove_model_tag(conn: &Connection, model_id: &str, tag: &str) -> Result<()>;
pub fn set_model_tags(conn: &Connection, model_id: &str, tags: &[String]) -> Result<()>;
```

`CreateProviderReq` / `UpdateProviderPatch` / `CreateModelReq` / `UpdateModelPatch` 是 store 层输入 DTO（不含 `id` / `created_at_ms` / `updated_at_ms`，由 store 生成）。

### 读取接口

```rust
/// 返回 provider 列表，api_key 替换为 has_api_key: bool。
pub fn list_providers(conn: &Connection) -> Result<Vec<ProviderSummary>>;

/// 统一模型目录查询。CLI/GUI/路由层共用此接口。
pub fn list_models_filtered(
    conn: &Connection,
    filter: ModelCatalogFilter,
) -> Result<Vec<ModelCatalogEntry>>;

pub struct ModelCatalogFilter {
    pub provider_id: Option<String>,
    pub tags: Vec<String>,           // 多 tag = AND 语义
    pub include_disabled: bool,      // false = 只返回 enabled model
}

pub fn list_tags(conn: &Connection) -> Result<Vec<String>>;
pub fn list_models_by_provider(conn: &Connection, provider_id: &str) -> Result<Vec<ModelCatalogEntry>>;
```

### `list_models_filtered` SQL 策略

单条 SQL 用 `LEFT JOIN` + `GROUP_CONCAT` 聚合 tags，避免 N+1：

```sql
SELECT
    p.id AS provider_id, p.name AS provider_name,
    p.provider_kind, p.enabled AS provider_enabled,
    m.id AS model_db_id, m.model_id, m.enabled AS model_enabled,
    COALESCE(GROUP_CONCAT(mt.tag, ','), '') AS tags_csv
FROM models m
JOIN providers p ON p.id = m.provider_id
LEFT JOIN model_tags mt ON mt.model_id = m.id
WHERE (:include_disabled OR m.enabled = 1)
  AND (:provider_id IS NULL OR m.provider_id = :provider_id)
  AND (:tag_count = 0 OR m.id IN (
      SELECT model_id FROM model_tags
      WHERE tag IN (:tags)
      GROUP BY model_id
      HAVING COUNT(DISTINCT tag) = :tag_count
  ))
GROUP BY m.id
ORDER BY p.name, m.model_id;
```

多 tag AND 语义：子查询 `HAVING COUNT(DISTINCT tag) = :tag_count`。tags_csv 在 Rust 层 split 成 `Vec<String>`。

### api_key 隔离

- `list_providers` 返回 `ProviderSummary`（`has_api_key: bool`），不返回原值。
- `update_provider_api_key` 是唯一写入 api_key 原值的入口。
- 读取 provider 单条时也不返回 api_key（DTO 层 mask）。

## 7. Runtime 集成

### DTO 变更（`crates/busytok-protocol/src/dto.rs`）

**删除字段**：`ProviderDto` / `ProviderCreateRequestDto` / `ProviderUpdateRequestDto` 中的 `api_key_env_name` / `base_url_env_name` / `models`。

**修改后的 Provider DTO**：
```rust
pub struct ProviderDto {
    pub id: String,
    pub name: String,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub enabled: bool,
    pub has_api_key: bool,
}

pub struct ProviderCreateRequestDto {
    pub name: String,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub enabled: Option<bool>,
    pub api_key: Option<String>,
}

pub struct ProviderUpdateRequestDto {
    pub id: String,
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub enabled: Option<bool>,
    pub api_key: Option<Option<String>>,  // None=不改, Some(None)=清除, Some(Some(k))=更新
}
```

**新增 Model DTO**：
```rust
pub struct ModelCatalogEntryDto {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_kind: String,
    pub provider_enabled: bool,
    pub model_db_id: String,
    pub model_id: String,
    pub model_enabled: bool,
    pub tags: Vec<String>,
}

pub struct ModelCreateRequestDto {
    pub provider_id: String,
    pub model_id: String,
    pub enabled: Option<bool>,
    pub tags: Option<Vec<String>>,
}

pub struct ModelUpdateRequestDto {
    pub id: String,
    pub model_id: Option<String>,
    pub enabled: Option<bool>,
}

pub struct ModelTagUpdateDto {
    pub model_id: String,  // model DB id
    pub tags: Vec<String>, // 全量覆盖语义
}
```

### RPC 方法（supervisor.rs）

**保留**（签名变更）：
- `provider.list` → 返回 `Vec<ProviderDto>`（无 models 字段）
- `provider.create` → 接收 `ProviderCreateRequestDto`，生成 UUID，写 SQL
- `provider.update` → 接收 `ProviderUpdateRequestDto`，patch SQL
- `provider.delete` → 删 SQL provider（CASCADE 删 model + tag）
- `provider.test_connection` → 用 SQL 中的 api_key + base_url 测试

**新增**：
- `model.list` → 参数 `{provider_id?, tags?, include_disabled?}`，返回 `Vec<ModelCatalogEntryDto>`
- `model.create` → 接收 `ModelCreateRequestDto`，生成 UUID，写 SQL，可选批量 insert tags
- `model.update` → 接收 `ModelUpdateRequestDto`，patch SQL
- `model.delete` → 删 SQL model（CASCADE 删 tag）
- `model.tags.update` → 接收 `ModelTagUpdateDto`，全量覆盖 tags（差集 add/remove）

### Sidecar env 注入（`crates/busytok-subagent/src/sidecar/pool.rs`）

**删除**：
- `SidecarConfig.api_key_env_name` / `base_url_env_name` 字段
- pool.rs:237-270 中动态构造 env name 的逻辑

**简化为**：
```rust
let provider = (self.providers)(provider_id)?;
let api_key = provider.api_key
    .ok_or_else(|| SidecarError::Spawn("no API key"))?;

let mut config = self.base_config.clone();
config.provider_id = provider_id.to_string();
config.env.insert("OPENAI_API_KEY".to_string(), api_key);
config.env.insert("OPENAI_BASE_URL".to_string(), provider.base_url);
```

固定两个 env name。`providers` 闭包返回的 `Provider` 需包含 `api_key` 字段（从 SQL 读，不走 keychain）。

## 8. CLI 命令

在 `apps/cli/src/main.rs` 的 `Command` enum 新增 `Catalog`：

```rust
Catalog {
    /// 按 provider ID 过滤
    #[arg(long)]
    provider: Option<String>,
    /// 按 tag 过滤（可重复，多 tag = AND 语义）
    #[arg(long = "tag")]
    tags: Vec<String>,
    /// 包含已禁用的模型
    #[arg(long)]
    all: bool,
    /// JSON 输出（默认 table）
    #[arg(long)]
    json: bool,
},
```

Handler（`apps/cli/src/commands.rs`）调用 `model.list` RPC，table 模式输出对齐表格，json 模式输出 pretty JSON。

Table 输出格式：
```
PROVIDER       MODEL              ENABLED  TAGS
openai         gpt-4o             yes      fast, vision
openai         o1-preview         yes      reasoning
deepseek       deepseek-chat      yes      cheap
deepseek       deepseek-reasoner  no       reasoning
```

不引入第三方 table 库，用 `println!` + 格式化对齐（与现有 subagent 命令风格一致）。

## 9. GUI 改造

### ProvidersPage.tsx

**删除**：`ProviderForm` 中的 `api_key_env_name` / `models` / `id` 输入框；`ProviderRow` 中的 models 标签展示。

**保留**：provider 的 name / base_url / api_key / enabled / test_connection / delete。

**新增**：在 provider 列表下方渲染 `<ModelsSection selectedProviderId={selectedProviderId} />`。

### ModelsSection 组件（`apps/gui/src/components/ModelsSection.tsx`）

展示选中 provider 下的模型列表：
- 新增模型表单：model_id 输入框 + enabled toggle + tags 输入框
- 每行模型：model_id + enabled toggle + tags 标签 + edit/delete 按钮
- edit 模式：tags 逗号分隔输入框，全量提交

### ProfilesSection.tsx

当前读 `provider.models`（字符串数组）做下拉。改为从 model catalog 查询：
```tsx
const { data: catalog } = useQuery({
  queryKey: ['models', editProviderId],
  queryFn: () => client.modelList({ provider_id: editProviderId, include_disabled: false }),
});
const availableModels = catalog?.map(m => m.model_id) ?? [];
```

### 前端 API client（`busytokClient.ts`）

新增 `modelList` / `modelCreate` / `modelUpdate` / `modelDelete` / `modelTagsUpdate` 方法。删除 `ProviderDto` 中的 `api_key_env_name` / `base_url_env_name` / `models` TypeScript 类型。

## 10. 清理清单

### 删除 keychain 全链路
- `crates/busytok-config/src/providers.rs`：删除 `KEYCHAIN_SERVICE`、`ProviderCredentialStore` struct + impl
- `crates/busytok-config/src/lib.rs`：从 `pub use` 移除 `ProviderCredentialStore`
- `crates/busytok-config/Cargo.toml`：删除 `keyring = "4"` 依赖
- `crates/busytok-config/src/providers.rs`：删除 keychain round-trip 测试
- `crates/busytok-config/tests/coverage_gaps_config.rs`：删除 ProviderCredentialStore 测试

### 删除旧 provider 模型
- `ProviderConfig` struct **整体删除**：被 `busytok-domain` 中的新 `Provider` 替代。删除 `crates/busytok-config/src/providers.rs` 中 `ProviderConfig` 定义及其 `Serialize/Deserialize` derive（keychain 删除后该文件只剩 `ProviderKind`，也迁移到 `busytok-domain`，文件可删除）
- `ProviderKind` enum 从 `busytok-config` 迁移到 `busytok-domain`（新 `Provider` 引用它）。`busytok-config` 不再 re-export
- `BusytokSettings`：删除 `providers: Vec<ProviderConfig>` 字段（`settings.toml` 不再存 provider 数据）
- `SidecarConfig`：删除 `api_key_env_name` / `base_url_env_name` 字段

### 残留 grep 断言
测试中断言以下字符串在 `src/` 中 0 匹配：
- `api_key_env_name`
- `base_url_env_name`
- `ProviderCredentialStore`
- `ProviderConfig`（整体删除，被 `busytok-domain::Provider` 替代）
- `keyring`
- `KEYCHAIN_SERVICE`

## 11. 观测性

遵循现有 `tracing` + `event_code` 风格：

```rust
// provider CRUD
info!(event_code = "provider.created", provider_id = %id, "provider created");
info!(event_code = "provider.updated", provider_id = %id, "provider updated");
info!(event_code = "provider.deleted", provider_id = %id, "provider deleted");

// model CRUD
info!(event_code = "model.created", model_id = %id, provider_id = %pid, "model created");
info!(event_code = "model.updated", model_id = %id, "model updated");
info!(event_code = "model.deleted", model_id = %id, "model deleted");

// tag 变更
info!(event_code = "model.tag_added", model_id = %id, tag = %tag, "tag added");
info!(event_code = "model.tag_removed", model_id = %id, tag = %tag, "tag removed");

// catalog 查询
debug!(event_code = "model.catalog.listed", filter_provider_id = ?pid, filter_tags = ?tags, "catalog listed");

// SQL 错误
error!(event_code = "provider.sql_write_failed", provider_id = %id, error = %e, "SQL write failed");
error!(event_code = "model.sql_read_failed", error = %e, "SQL read failed");
```

**严禁**：任何日志 field 中出现 `api_key` 值。

## 12. 测试计划

覆盖率目标：变更文件行覆盖率 ≥ 90%。

### Store 层（`crates/busytok-store/tests/provider_catalog.rs`）
- create provider with/without api_key
- update provider metadata + api_key
- delete provider cascades models + tags
- list_providers returns `has_api_key` not raw key
- create model under provider
- reject duplicate `(provider_id, model_id)`
- enable/disable model
- delete model cascades tags
- add/remove tag, duplicate tag no-op
- list_tags distinct
- filter by single tag
- filter by multiple tags AND
- filter by provider
- include_disabled semantics

### Runtime 层（`crates/busytok-runtime/tests/supervisor_control.rs`）
- provider.create/update/delete RPC
- model.create/update/delete RPC
- model.tags.update RPC（全量覆盖）
- model.list RPC with filter
- sidecar spawn 注入 `OPENAI_API_KEY` + `OPENAI_BASE_URL`
- 断言不依赖 `api_key_env_name` / keychain

### CLI 层（`apps/cli/src/commands.rs` 测试）
- catalog table 输出格式
- catalog json 输出
- catalog --provider 过滤
- catalog --tag 单 tag
- catalog --tag --tag 多 tag AND
- catalog --all 显示禁用

### 残留断言（`crates/busytok-config/tests/`）
- grep 测试：`api_key_env_name` / `base_url_env_name` / `ProviderCredentialStore` / `keyring` 在 src/ 中 0 匹配

## 13. 实施阶段

| 阶段 | 内容 | 性质 |
|------|------|------|
| P1 | 新 domain model + v6 schema + store repository | 纯增量，不碰旧代码 |
| P2 | supervisor RPC 切到新 store + sidecar 固定 env + 删 keychain + 删旧字段 | "翻转"点 |
| P3 | CLI catalog 命令 + GUI ProvidersPage/ModelsSection 改造 | 消费层 |
| P4 | 日志事件 + 残留 grep 清理 + 测试补全 | 收尾 |

## 14. 验收标准

- provider / model / tag 全量由 SQL 管理
- provider 不再包含模型字符串列表
- API key 不再依赖 Keychain
- sidecar 固定使用 `OPENAI_API_KEY` / `OPENAI_BASE_URL`
- CLI 可以一次性列出所有模型及其 provider 和 tags
- 模型可打多个 tag
- 可查询某 tag 或多 tag 下的模型
- GUI 和 CLI 都基于统一 model catalog 读取
- 代码库中不再残留旧 provider env-name 设计
- 变更文件行覆盖率 ≥ 90%

## 15. 非目标

- 历史兼容迁移
- 数据库加密
- 自动路由策略引擎
- tag 字典治理
- 复杂模型评分排序
- 多协议 provider 全量接入设计扩展
