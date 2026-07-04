# Subagent 绑定固定 Provider 与 Model，并接入 Pi SDK 多 API 形状

**Date:** 2026-07-04  
**Status:** approved  
**Requires:** Phase 1–5 (merged), `feat/provider-model-catalog-refactor` branch

## 1. Motivation

当前系统的路由真相分散在三处：subagent 的 `default_model`、profile 的 `provider_id` / `model`、以及 sidecar 的 `OPENAI_*` env var 注入。这导致：

1. subagent 只部分固化了 model，没有固化 provider — 执行时 provider 仍从 profile 现读，配置可漂移
2. sidecar worker 只支持固定 `OPENAI_API_KEY` / `OPENAI_BASE_URL` 注入，无法表达 Anthropic API 形状
3. Pi SDK session 的 model 在 `createAgentSession()` 时确定，但当前 sidecar 既不传 `apiKey` 也不传 `baseUrl`，SDK 只能靠自身 fallback 机制

本次重构将路由真相唯一化到 `subagent.bound_provider_id + bound_model_id`，并接入 Pi SDK 的 `AuthStorage.inMemory` + `ModelRegistry.create` + `registerProvider` 机制。

## 2. 数据模型

### 2.1 ProviderKind 扩展

```rust
pub enum ProviderKind {
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    #[serde(rename = "anthropic_compatible")]
    AnthropicCompatible,
}
```

SQL `providers.provider_kind` 列已为 `TEXT NOT NULL DEFAULT 'openai_compatible'`，无需 schema 变更，store 层解析时增加 `"anthropic_compatible"` 支持。

### 2.2 `models` 表增加 metadata

Pi SDK 的 `registerProvider(models)` 需要完整 model 定义（不只 `id`）。新增 migration `0007_subagent_route_binding_and_model_metadata.sql`：

```sql
ALTER TABLE models ADD COLUMN display_name TEXT;
ALTER TABLE models ADD COLUMN reasoning INTEGER NOT NULL DEFAULT 0;
ALTER TABLE models ADD COLUMN context_window INTEGER;
ALTER TABLE models ADD COLUMN max_tokens INTEGER;
```

- SQL 允许 `NULL`（migration 安全）
- 运行时 `create_model` RPC 必须要求 `context_window` / `max_tokens` 有值（`display_name` / `reasoning` 可选）
- `reasoning` 默认 `0`（false）
- `display_name` 可选

同步更新以下类型/查询以读写新列：
- `busytok-domain::provider_catalog::Model` domain struct（新增 `display_name: Option<String>` / `reasoning: bool` / `context_window: Option<i64>` / `max_tokens: Option<i64>`）
- `busytok-store::provider_catalog::CreateModelReq`（新增对应字段）
- `busytok-store::provider_catalog::UpdateModelPatch`（新增可选 patch 字段）
- `busytok-store::provider_catalog` 的 `create_model` / `get_model_by_id` / `get_model_by_provider_and_model_id` / `list_models_filtered` / `row_to_model` 查询
- `busytok-protocol::dto::ModelCreateRequestDto`（新增 `context_window` / `max_tokens` 必填，`display_name` / `reasoning` 可选）
- `busytok-protocol::dto::ModelUpdateRequestDto`（新增可选 patch 字段）
- `busytok-protocol::dto::ModelCatalogEntryDto`（新增 `display_name` / `reasoning` / `context_window` / `max_tokens` 字段）

migration 注册时同步将 `busytok-store::schema::SCHEMA_VERSION` 从 `6` bump 到 `7`，并在 `migrations()` 末尾追加 `(7, SUBAGENT_ROUTE_BINDING_AND_MODEL_METADATA_SQL)`。

### 2.3 `subagent_logical_subagents` 重建

项目未上线，直接重建表，带正确的 `NOT NULL` 约束，删除 `default_model` 列。存量数据可丢弃，无需搬数据。

migration 在 `unchecked_transaction` 内 `execute_batch`，且 `PRAGMA foreign_keys = ON` 在事务内无法关闭。`subagent_logical_subagents` 被 `subagent_memory` / `subagent_tasks` / `subagent_harness_bindings` / `subagent_usage_records` 通过 FK 引用（均无 `ON DELETE CASCADE`），直接 `DROP TABLE` 在有存量数据时会因 FK 约束失败。因此先按 FK 依赖顺序 drop 所有子表，再 drop + recreate 主表，最后按 `0003_subagent.sql` 中的定义 recreate 子表（schema 不变，仅清除存量数据）：

```sql
-- 1. Drop child tables (FK references to subagent_logical_subagents, no cascade)
DROP TABLE IF EXISTS subagent_usage_records;
DROP TABLE IF EXISTS subagent_harness_bindings;
DROP TABLE IF EXISTS subagent_tasks;
DROP TABLE IF EXISTS subagent_memory;

-- 2. Drop and recreate parent table (new schema: bound_provider_id + bound_model_id NOT NULL, no default_model)
DROP TABLE IF EXISTS subagent_logical_subagents;

CREATE TABLE subagent_logical_subagents (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    project_id TEXT NOT NULL,
    repo_path TEXT NOT NULL,
    repo_hash TEXT NOT NULL,
    branch TEXT,
    intent TEXT,
    default_profile TEXT NOT NULL,
    bound_provider_id TEXT NOT NULL,
    bound_model_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'cold'
        CHECK (status IN ('hot', 'warm', 'cold', 'deleted')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    last_active_at_ms INTEGER
);

CREATE INDEX idx_subagent_logical_project
    ON subagent_logical_subagents(project_id, repo_hash, status);
CREATE INDEX idx_subagent_logical_last_active
    ON subagent_logical_subagents(last_active_at_ms);
CREATE UNIQUE INDEX idx_subagent_unique_active_name
    ON subagent_logical_subagents(project_id, repo_hash, name)
    WHERE status != 'deleted';

-- 3. Recreate child tables (same schema as 0003_subagent.sql; subagent_resource_events 无 FK 引用但一并重建)
--    完整 CREATE TABLE 语句复用 0003_subagent.sql 中 subagent_memory / subagent_tasks /
--    subagent_harness_bindings / subagent_usage_records / subagent_resource_events 的定义
--    及其索引。
```

不使用 `DEFAULT ''` 过渡态，不使用 `ALTER TABLE ADD COLUMN` + `DROP COLUMN`（无法添加 `NOT NULL` 无默认值的列）。

### 2.4 `LogicalSubagent` domain struct

```rust
pub struct LogicalSubagent {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub repo_path: String,
    pub repo_hash: String,
    pub branch: Option<String>,
    pub intent: Option<String>,
    pub default_profile: String,
    pub bound_provider_id: String,
    pub bound_model_id: String,
    pub status: SubagentStatus,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_active_at_ms: Option<i64>,
}
```

删除 `default_model` 字段。所有引用 `default_model` 的 DTO、store row、resolver、tests 同步清除，包括：
- `busytok-store::repository::SubagentLogicalSubagentRow`（`default_model` 字段删除，新增 `bound_provider_id: String` / `bound_model_id: String`）
- `busytok-store::subagent_queries` 的 `upsert_logical_subagent` / `get_logical_subagent` / `list_active_by_repo` / `find_by_name_in_repo` / `list_filtered` 的 SQL 列列表
- `busytok-protocol::dto::SubagentDetailDto`（`default_model` 字段删除，新增 `bound_provider_id: String` / `bound_model_id: String`）
- `packages/busytok-protocol-types/src/generated.ts`（重新生成）

### 2.5 Profile 降级为纯行为模板

`SubagentProfileConfig` 删除 `provider_id` 和 `model` 字段，仅保留：

```rust
pub struct SubagentProfileConfig {
    pub write_access: bool,
    pub tools: Vec<String>,
    pub context_budget_tokens: u32,
    pub timeout_seconds: u64,
}
```

`SubagentModelsConfig`（`default_cheap_model` / `default_review_model` / `default_reasoning_model` / `default_coder_model`）整体删除。`profile_model()` 方法删除。`profile_create` / `profile_update` 的所有 provider/model whitelist 校验删除。

同步更新以下 DTO 以删除 `provider_id` / `model` 字段：
- `busytok-protocol::dto::ProfileDto`（删除 `provider_id` / `model`）
- `busytok-protocol::dto::ProfileCreateRequestDto`（删除 `provider_id` / `model`）
- `busytok-protocol::dto::ProfileUpdateRequestDto`（删除 `provider_id` / `model` patch 字段）
- `busytok-config::lib::default_profiles()` 中每个内置 profile 的构造（不再设置 `model` / `provider_id`）
- `packages/busytok-protocol-types/src/generated.ts`（重新生成）

## 3. 创建 subagent 的新语义

### 3.1 `resolve_by_name` 签名

```rust
pub fn resolve_by_name(
    db: &Database,
    name: &str,
    cwd: &str,
    default_profile: &str,
    bound_provider_id: &str,
    bound_model_id: &str,
) -> Result<Resolved>
```

删除 `default_model: Option<&str>` 参数。

### 3.2 创建时校验链

1. `bound_provider_id` 必须在 `providers` 表中存在且 `enabled = true`
2. `bound_model_id` 必须在该 provider 下存在且 `enabled = true`
3. 校验失败返回明确错误：`provider not found` / `provider disabled` / `model not found in provider` / `model disabled`

### 3.3 `delegate()` 请求 DTO

`SubagentDelegateRequestDto`（`busytok-protocol::dto`）新增：

```rust
pub bound_provider_id: Option<String>,
pub bound_model_id: Option<String>,
```

`busytok-subagent::models::DelegateRequest`（内部 struct，由 DTO 转换而来）同步新增相同字段。

语义（条件必填）：

- **复用已有 subagent**（`subagent_id` 或 `subagent_name + cwd` 命中）：忽略这两个字段，真实路由只读 DB 已绑定值，不做隐式 rebind
- **创建新 subagent**（`subagent_name + cwd` 未命中）：这两个字段必须提供，且通过 provider/model 校验
- **两字段只提供一个**：直接报 `bound_provider_id and bound_model_id must be provided together`

未来若支持 rebind，走独立 RPC。

### 3.4 DB 写入

`create_subagent()` 写入 `default_profile`（NOT NULL）、`bound_provider_id`（NOT NULL）、`bound_model_id`（NOT NULL）。不再写 `default_model`。

### 3.5 runtime 不从 profile 派生 model/provider

- runtime 不再根据 `default_profile` 派生任何默认 model/provider
- `default_profile` 仅保留行为模板用途（`write_access` / `tools` / `context_budget_tokens` / `timeout_seconds`）
- GUI 层可从 provider 的 model 列表中选第一个 enabled model 作为 UI 默认建议值，runtime 层不再 hardcode

## 4. delegate 执行路径改造

### 4.1 模型解析链（唯一化）

```
effective_model_id = task.model_override.unwrap_or(subagent.bound_model_id)
```

- `task.model_override` 仅用于单次任务覆盖，不回写 subagent
- `task.model_override == Some(...)` 时，也必须 against `bound_provider_id` 重新做 whitelist + enabled 校验
- `subagent.bound_model_id` 是长期绑定模型
- `profile_model()` 不再出现在执行路径的任何分支

### 4.2 Provider 解析链（唯一化）

```
provider_id = subagent.bound_provider_id
```

从 `profile_cfg.provider_id` 读取的逻辑删除。`ExecutorInput.provider_id` 改为 `String`（不再是 `Option<String>`）。`SidecarTaskExecutor` 中 "profile not bound to a provider" 的错误分支删除。

### 4.3 有效执行路由校验链

`execute_task()` 在拿到 `subagent.bound_provider_id` + `effective_model_id` 后，执行前校验：

1. `effective_model_id = task.model_override.unwrap_or(subagent.bound_model_id)`
2. `get_provider_with_secret(bound_provider_id)` — 必须存在，否则 `bound provider not found`
3. `provider.enabled == true` — 否则 `bound provider disabled`
4. `provider.api_key` 必须存在且非空 — 否则 `bound provider missing api key`
5. `get_model_by_provider_and_model_id(bound_provider_id, effective_model_id)` — 必须存在，否则 `bound model not found in provider`
6. `model.enabled == true` — 否则 `bound model disabled`

校验失败时 task 直接标记为 failed，返回明确错误信息。

### 4.4 `ExecutorInput` 变更

```rust
pub struct ExecutorInput {
    // ... 现有字段 ...
    pub provider_id: String,           // Option<String> → String
    pub provider_kind: ProviderKind,   // 新增
    pub provider_base_url: String,     // 新增
    pub provider_api_key: String,      // 新增（瞬时执行态数据，不写回 task row，不进日志明文，不进任何 DTO/response/diagnostic）
    pub model: String,                 // Option<String> → String
}
```

`provider_api_key` 是瞬时执行态数据：
- 不写回 task row
- 不进日志明文
- 不进任何 DTO / response / diagnostic payload

## 5. sidecar / Pi SDK 接入

### 5.1 核心原则

- `AuthStorage.inMemory` 是 secret 唯一来源
- `ModelRegistry.create` + `registerProvider` 是 provider/model runtime 唯一来源
- `turn_auto` params 是创建新 session 时的权威输入
- `process.env` 不参与 provider secret 传递
- `registry.find(providerId, modelId)` 是唯一 model lookup 方式

### 5.2 `turn_auto` RPC params 扩展

```typescript
type ProviderKind = "openai_compatible" | "anthropic_compatible";

export interface TurnAutoParams {
  // ... 现有字段 ...
  provider_kind: ProviderKind;
  provider_base_url: string;
  provider_api_key: string;
  // model 字段已存在，继续沿用
}
```

`provider_id` 字段保留（用于 usage attribution 和日志）。职责划分：
- `provider_kind + provider_base_url + provider_api_key` 用于**注册 provider runtime**
- `provider_id + model` 用于**在注册后的 registry 中做精确查找**

### 5.3 `provider_kind` → Pi SDK API 映射

| `provider_kind` (Rust) | Pi SDK `api` value |
|------------------------|---------------------|
| `openai_compatible` | `"openai-completions"` |
| `anthropic_compatible` | `"anthropic-messages"` |

### 5.4 session miss 时的 Pi SDK 环境构造

`defaultSessionFactory` 在 session miss 时：

```typescript
// 1. AuthStorage — in-memory，secret 唯一来源
const authStorage = AuthStorage.inMemory({
  [providerId]: { type: "api_key", key: providerApiKey },
});

// 2. ModelRegistry — in-memory，无文件 I/O（与现有 `ModelRegistry.create(AuthStorage.inMemory())` API 一致）
const registry = ModelRegistry.create(authStorage);

// 3. 动态注册 provider
registry.registerProvider(providerId, {
  baseUrl: providerBaseUrl,
  api: piApiValue,                    // 映射后的 "openai-completions" / "anthropic-messages"
  apiKey: "__busytok_runtime__",      // 占位符，真实 key 来自 authStorage
  models: [{
    id: modelId,
    name: modelDisplayName ?? modelId,
    reasoning: modelReasoning,
    input: ["text"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: modelContextWindow,
    maxTokens: modelMaxTokens,
  }],
});

// 4. 精确查找 model
const model = registry.find(providerId, modelId);

// 5. 创建 session
const { session } = await createAgentSession({
  cwd,
  tools,
  model,
  authStorage,
  modelRegistry: registry,
});
```

### 5.5 session hit 时忽略路由参数

`SessionPool.ensure()` hit 分支保持不变——只按 `logical_subagent_id` 命中，命中后直接复用已有 session，忽略 `turn_auto` 携带的 `provider_kind` / `provider_base_url` / `provider_api_key`。

### 5.6 `resolveModelObject` 重写

删除全局缓存 `cachedRegistry`。`resolveModelObject` 逻辑内联到 `defaultSessionFactory` 中，每次 session 创建时构造独立的 registry。

### 5.7 `inject_provider_env` 删除

`pool.rs` 的 `inject_provider_env` 函数直接删除。`turn_auto` params 是 provider runtime 配置的唯一通道。`ProviderRuntimeEntry` 保持现有 3 字段（`provider_id` / `api_key` / `base_url`），不扩展——它不是 provider runtime secret/config 的权威来源，sidecar session 创建时的权威来源是 `turn_auto` 路由参数。

## 6. turn_auto / session 创建语义

### 6.1 `CreateSessionOpts` 扩展

```typescript
export interface CreateSessionOpts {
  cwd: string;
  model: string;                      // Option<string> → string
  provider_id: string;                // Option<string> → string
  provider_kind: ProviderKind;        // 新增
  provider_base_url: string;          // 新增
  provider_api_key: string;           // 新增
  model_reasoning: boolean;           // 新增，必填
  model_context_window: number;       // 新增，必填
  model_max_tokens: number;           // 新增，必填
  model_display_name?: string;        // 新增，可选（fallback 到 model）
  tools?: string[];
}
```

model metadata（`model_reasoning` / `model_context_window` / `model_max_tokens`）从 Rust 侧 `models` 表读取，通过 `turn_auto` params 透传到 sidecar。`model_display_name` 可选，sidecar 里 fallback 到 `model`。

### 6.2 `turn_auto` handler 改造

`turnAutoHandlerWithPool` 从 `TurnAutoParams` 中提取路由参数，构造 `CreateSessionOpts` 传给 `pool.ensure()`。hit 时 `pool.ensure()` 不消费这些参数；miss 时 `defaultSessionFactory` 消费它们。

### 6.3 硬约束：热会话存活期间不允许 rebind

同一 `logical_subagent_id` 的 `bound_provider_id` + `bound_model_id` 在热会话存活期间不允许变化。本期不提供 rebind RPC。如果未来支持 rebind，必须同时做 session 失效或命中条件升级。

## 7. provider 更新后的行为

### 7.1 provider 配置变更的影响

provider 是共享连接对象。以下变更影响所有绑定该 provider 的 subagent：

- 修改 `api_key` → kill worker，后续新 session 使用新 key
- 修改 `base_url` → kill worker，后续新 session 使用新 URL
- 修改 `provider_kind` → kill worker，后续新 session 使用新 API 形状
- 禁用 provider（`enabled=false`）→ kill worker，后续 delegate 校验失败 `bound provider disabled`
- 重新启用 provider（`enabled=true`）→ 确保 worker 不存在，下次 delegate 重建 worker/session
- 删除 provider → 后续 delegate 校验失败 `bound provider not found`

### 7.2 强制 kill worker 机制

复用现有 `provider_changed` → `update_provider_and_kill_old` 机制。provider 的 `api_key` / `base_url` / `provider_kind` / `enabled` 变更时触发。kill 后 SessionPool 清空，下次 delegate 必然 miss。

### 7.3 model 变更的影响

- 禁用 model → 后续 delegate 在校验链第 5/6 步失败
- 删除 model → 后续 delegate 在校验链第 5 步失败
- model metadata 变更（`context_window` 等）→ **不触发 worker kill**，只影响后续 miss 创建的新 session

### 7.4 绑定不自动漂移

- provider 更新不改变 subagent 的 `bound_provider_id`
- model 禁用/删除不改变 subagent 的 `bound_model_id`
- subagent 绑定创建后稳定不变，本期不提供 rebind

## 8. 本次明确不做

- 不做 subagent route snapshot
- 不做 provider revision/version 系统
- 不做 SessionPool 命中条件升级
- 不做 subagent rebind UI / RPC
- 不做 profile 工具配置体系重构
- 不做 `input_types` / `cost` 字段（model metadata 仅限 `display_name` / `reasoning` / `context_window` / `max_tokens`）

## 9. 验证要求

### 9.1 数据模型测试

1. 创建新 subagent 时，`bound_provider_id` + `bound_model_id` 正确写入数据库
2. `default_model` 列已删除，相关 DTO/struct 无此字段
3. `ProviderKind::AnthropicCompatible` 序列化/反序列化为 `"anthropic_compatible"`
4. `models` 表 metadata 字段正确读写

### 9.2 创建路径测试

5. 创建时 `bound_provider_id` 不存在 → 失败 `provider not found`
6. 创建时 provider `enabled=false` → 失败 `provider disabled`
7. 创建时 `bound_model_id` 不在该 provider 下 → 失败 `model not found in provider`
8. 创建时 model `enabled=false` → 失败 `model disabled`
9. 创建时只提供 `bound_provider_id` 不提供 `bound_model_id` → 失败 `must be provided together`
10. 复用已有 subagent（name 路径）时传入 `bound_provider_id` / `bound_model_id` → 忽略，不覆盖 DB 值
11. 复用已有 subagent（`subagent_id` 路径）时传入 `bound_provider_id` / `bound_model_id` → 忽略，不覆盖 DB 值
12. `create_model` 对非内置模型缺少 `context_window` / `max_tokens` → 拒绝

### 9.3 执行路径测试

13. 已存在 subagent delegate 时，读取的是 `subagent.bound_provider_id`，不是 profile
14. 修改 profile 配置后，老 subagent 执行结果不受影响
15. `task.model_override` 覆盖 `bound_model_id` 时，用 override 值做 whitelist 校验
16. `effective_model_id` 不在 provider 白名单 → 失败 `bound model not found in provider`
17. `bound_provider_id` disabled → 失败 `bound provider disabled`
18. `bound_provider_id` deleted → 失败 `bound provider not found`
19. provider `api_key` 为空 → 失败 `bound provider missing api key`

### 9.4 provider 更新测试

20. 修改 provider `api_key` 后，后续执行使用新 key（新 session）
21. 修改 provider `base_url` 后，后续执行使用新 URL
22. 修改 provider `provider_kind` 后，后续执行使用新 API 形状
23. 禁用 provider → kill worker + delegate 失败
24. 重新启用 provider → 下次 delegate 重建 worker/session
25. model metadata 变更不触发 worker kill

### 9.5 sidecar / Pi SDK 测试

26. `provider_kind=openai_compatible` 时，sidecar 用 `api: "openai-completions"` 注册 provider
27. `provider_kind=anthropic_compatible` 时，sidecar 用 `api: "anthropic-messages"` 注册 provider
28. 首次 delegate 把正确 model + metadata 传入 `createAgentSession()`
29. 复用已有 hot session 时，继续沿用原有 model（忽略新路由参数）
30. `AuthStorage.inMemory` 是 secret 唯一来源，`process.env` 不参与 secret 传递
31. `ModelRegistry.create` + `registerProvider` 是 provider runtime 唯一来源，无文件 I/O

### 9.6 回归测试

32. 现有 `provider_changed_removes_worker_then_respawns` 测试继续通过
33. 现有 `e2e_multi_provider_creates_separate_workers` 测试继续通过
34. 现有 `e2e_auth_failure_kills_worker` 测试继续通过

## 10. 约束与已知边界

- 同一个 `logical_subagent_id` 命中已有 hot session 时，会继续复用原 session
- 如果未来要支持同一 subagent 的 provider/model 重绑，则必须同时做 session 失效或命中条件升级
- 因此本次实现中，subagent 的 `bound_provider_id + bound_model_id` 应视为创建后稳定不变
- 不提供 rebind 能力，本期不允许从产品入口修改这两个字段

本期设计成立的前提：
- 老 subagent 绑定不改
- 新 subagent 可选不同 provider/model
- provider 的 key / URL / kind 可更新（通过 kill worker 强制后续 miss 生效）
- model 绑定不可热切换
