# Price Catalog JSON 格式规范 (Schema v3)

## 文件位置

`crates/busytok-pricing/src/price_catalog.json`

## 完整示例

```json
{
  "schema_version": "3",
  "version": "2026-06-07",
  "updated": "2026-06-07",
  "aliases": {
    "claude-sonnet-4": "claude-sonnet-4-20250514",
    "claude-4-sonnet": "claude-sonnet-4-20250514",
    "gpt-5-codex": "gpt-5",
    "gpt-5.3-codex-spark": "gpt-5.3-codex"
  },
  "prices": [
    {
      "provider": "anthropic",
      "model": "claude-sonnet-4-20250514",
      "currency": "USD",
      "effective_date": "2025-05-14",
      "fast_multiplier": 6.0,
      "tiers": [
        {
          "from_tokens": 0,
          "input_per_million": 3.0,
          "output_per_million": 15.0,
          "cached_input_per_million": 0.3,
          "cache_write_per_million": 3.75,
          "cache_storage_per_million_hour": null,
          "reasoning_per_million": null
        },
        {
          "from_tokens": 200000,
          "input_per_million": 6.0,
          "output_per_million": 22.5,
          "cached_input_per_million": 0.6,
          "cache_write_per_million": 7.5,
          "cache_storage_per_million_hour": null,
          "reasoning_per_million": null
        }
      ]
    }
  ]
}
```

---

## 顶层字段

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `schema_version` | string | **是** | 固定值 `"3"` |
| `version` | string | **是** | 配置文件版本号，推荐 ISO date 格式 `YYYY-MM-DD`。每次修改必须更新 |
| `updated` | string | **是** | 最后更新时间，ISO date 格式 |
| `aliases` | object | **是** | 模型名映射表。key = 日志中出现的模型名，value = `prices[].model` 中的规范名。可为空 `{}` |
| `prices` | array | **是** | 模型定价列表，至少 1 条 |

---

## aliases（模型别名）

日志中记录的模型名可能与 catalog 中的规范名不同。别名用于建立映射关系。

```json
"aliases": {
  "claude-sonnet-4": "claude-sonnet-4-20250514",
  "claude-4-sonnet": "claude-sonnet-4-20250514",
  "gpt-5-codex": "gpt-5"
}
```

规则：
- key 是 Agent 日志中**实际出现的**模型名字符串（trim 后比对）
- value 必须**精确等于**某个 `prices[].model`
- 多个 key 可指向同一个 value（多对一）
- 不允许 key 重复
- 如果 value 指向的 model 不在 `prices` 中，整个配置文件加载失败

---

## prices[].ModelPrice

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `provider` | string | **是** | Provider 名称。**仅允许小写字母、连字符 `-`、下划线 `_`**（如 `anthropic`、`openai`、`google`、`deepseek`） |
| `model` | string | **是** | 规范模型名，**在 `prices[]` 中必须唯一**。aliases 指向此名称 |
| `currency` | string | **是** | 固定值 `"USD"` |
| `effective_date` | string | **是** | 该定价生效日期，ISO date `YYYY-MM-DD` |
| `fast_multiplier` | number \| null | **是** | Fast mode 加价倍数。不支持 fast mode 的模型填 `null`。若为 number 则必须 > 0 且有限（不能是 0、负数、NaN、Infinity） |
| `tier_mode` | string | 否 | 阶梯计价模式：`"marginal"`（默认）或 `"whole_request"`。见下方说明 |
| `tiers` | array | **是** | 阶梯定价数组，**至少 1 档**。第一档 `from_tokens` 必须为 `0`，后续严格递增 |

**不允许出现以下旧字段（v2 遗留）：**

```
input_per_million
output_per_million
cached_input_per_million
reasoning_per_million
input_per_million_above_200k
output_per_million_above_200k
cached_input_per_million_above_200k
```

所有价格字段已迁移到 `tiers[]` 内部。如果顶层出现这些字段，JSON 解析会直接报错。

---

## prices[].tiers[].PriceTier

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `from_tokens` | integer | **是** | 该档从多少个 token 开始适用。**第一档必须为 `0`**，后续必须严格递增（不能相等或递减） |
| `input_per_million` | number | **是** | 普通输入 token 每百万单价。**不能为 null**，必须 ≥ 0 且有限 |
| `output_per_million` | number | **是** | 输出 token 每百万单价。**不能为 null**，必须 ≥ 0 且有限 |
| `cached_input_per_million` | number \| null | **是** | Cache read / cache hit token 每百万单价。`null` 表示模型不支持 prompt caching |
| `cache_write_per_million` | number \| null | **是** | Cache write / cache creation token 每百万单价。`null` 表示不单独对 cache write 计费 |
| `cache_storage_per_million_hour` | number \| null | **是** | Cache storage 每百万 token 每小时的单价。目前所有模型填 `null`（数据源不暴露此信息） |
| `reasoning_per_million` | number \| null | **是** | Reasoning token 每百万单价。`null` 表示 reasoning 已包含在 output 计费中，不单独计价 |

所有 number 类型字段（如果非 null）必须 **≥ 0 且有限**（不能是 NaN 或 Infinity）。

---

## 阶梯定价规则

`tier_mode` 决定阶梯的应用方式。两种模式：

### `marginal`（默认）— 边际分段计价

每类 token **独立**按阈值分段计费。适用于 Anthropic、OpenAI 等。

例如：

```json
"tiers": [
  { "from_tokens": 0,      "input_per_million": 3.0 },
  { "from_tokens": 200000, "input_per_million": 6.0 }
]
```

含义：前 200,000 个 input token 按 $3.0/M 计价，超过 200,000 的部分按 $6.0/M 计价。

- 阈值**对每类 token 独立适用**（input 和 output 各自分段）
- 第一档从 0 开始，最后一档到无限大

### `whole_request` — 整请求选档计价

根据**总 prompt 大小**（`input_tokens`）选择一个档位，所有 token 类别统一使用该档的价格。适用于按整请求计价的模型。

例如第二档使用 `from_tokens: 200001`：

```json
"tier_mode": "whole_request",
"tiers": [
  { "from_tokens": 0,      "input_per_million": 1.25, "output_per_million": 10.0, "cached_input_per_million": 0.125 },
  { "from_tokens": 200001, "input_per_million": 2.50, "output_per_million": 15.0, "cached_input_per_million": 0.25 }
]
```

含义：如果 `input_tokens` ≤ 200,000，**所有** token（input、output、cached）按第一档价格计费。如果 ≥ 200,001，**所有** token 按第二档价格计费。不做分段。

- 选档依据是 `input_tokens`（`cached_input_tokens` 和 `cache_creation_tokens` 是其子集，不需要额外相加）
- 200,000 → 第一档（≤ 200k），200,001 → 第二档（> 200k）

### 通用规则

- 如果某档价格为 `null` 但该 token 类型数量 > 0，系统会记录 warn 日志并把该部分成本计为 0
- 如果 JSON 中不指定 `tier_mode`，默认为 `"marginal"`

---

## 不同模型配置示例

### 无阶梯模型（如 OpenAI）

```json
{
  "provider": "openai",
  "model": "gpt-5",
  "currency": "USD",
  "effective_date": "2025-08-07",
  "fast_multiplier": null,
  "tiers": [
    {
      "from_tokens": 0,
      "input_per_million": 1.25,
      "output_per_million": 10.0,
      "cached_input_per_million": 0.125,
      "cache_write_per_million": null,
      "cache_storage_per_million_hour": null,
      "reasoning_per_million": null
    }
  ]
}
```

### 200k 阶梯 + Cache Write 模型（如 Anthropic Claude）

```json
{
  "provider": "anthropic",
  "model": "claude-sonnet-4-20250514",
  "currency": "USD",
  "effective_date": "2025-05-14",
  "fast_multiplier": 6.0,
  "tiers": [
    {
      "from_tokens": 0,
      "input_per_million": 3.0,
      "output_per_million": 15.0,
      "cached_input_per_million": 0.3,
      "cache_write_per_million": 3.75,
      "cache_storage_per_million_hour": null,
      "reasoning_per_million": null
    },
    {
      "from_tokens": 200000,
      "input_per_million": 6.0,
      "output_per_million": 22.5,
      "cached_input_per_million": 0.6,
      "cache_write_per_million": 7.5,
      "cache_storage_per_million_hour": null,
      "reasoning_per_million": null
    }
  ]
}
```

### 512k 阶梯模型（如 MiniMax）

```json
{
  "provider": "minimax",
  "model": "minimax-m1",
  "currency": "USD",
  "effective_date": "2025-06-01",
  "fast_multiplier": null,
  "tiers": [
    {
      "from_tokens": 0,
      "input_per_million": 0.3,
      "output_per_million": 1.2,
      "cached_input_per_million": null,
      "cache_write_per_million": null,
      "cache_storage_per_million_hour": null,
      "reasoning_per_million": null
    },
    {
      "from_tokens": 512000,
      "input_per_million": 0.6,
      "output_per_million": 2.4,
      "cached_input_per_million": null,
      "cache_write_per_million": null,
      "cache_storage_per_million_hour": null,
      "reasoning_per_million": null
    }
  ]
}
```

### 整请求阶梯模型（如 OpenAI GPT-5.4）

```json
{
  "provider": "openai",
  "model": "gpt-5.4",
  "currency": "USD",
  "effective_date": "2026-03-05",
  "fast_multiplier": null,
  "tier_mode": "whole_request",
  "tiers": [
    {
      "from_tokens": 0,
      "input_per_million": 2.5,
      "output_per_million": 15.0,
      "cached_input_per_million": 0.25,
      "cache_write_per_million": null,
      "cache_storage_per_million_hour": null,
      "reasoning_per_million": null
    },
    {
      "from_tokens": 272001,
      "input_per_million": 5.0,
      "output_per_million": 22.5,
      "cached_input_per_million": 0.5,
      "cache_write_per_million": null,
      "cache_storage_per_million_hour": null,
      "reasoning_per_million": null
    }
  ]
}
```

注意：`from_tokens: 272001` 而非 `272000`，保证 ≤ 272k 的请求选第一档，> 272k 的请求选第二档。

---

## 校验规则（加载时自动检查，不通过则拒绝整个文件）

1. `schema_version` 必须为 `"3"`
2. `version` 非空
3. `prices` 至少包含 1 个模型
4. `prices[].model` 全局唯一（不允许重复）
5. `provider` 非空且仅含小写字母、`-`、`_`
6. `currency` 必须为 `"USD"`
7. `tiers` 非空数组
8. `tiers[0].from_tokens` 必须为 `0`
9. `tiers[].from_tokens` 严格递增
10. `input_per_million` 和 `output_per_million` 必须是非 null 的 number，≥ 0 且有限
11. 其他价格字段（cached/cache_write/cache_storage/reasoning）若为 number 则必须 ≥ 0 且有限
12. `aliases` 的每个 value 必须精确匹配某个 `prices[].model`
13. `fast_multiplier` 若非 null，必须 > 0 且有限
14. 顶层不允许出现 v2 旧字段（JSON 解析直接报错）
15. `tier_mode` 必须为 `"marginal"` 或 `"whole_request"`（JSON 解析校验），省略时默认 `"marginal"`

---

## 工程师任务清单

1. 网上搜索以下 provider 的**最新官方定价**，完善 `prices[]`：
   - **Anthropic**: Claude Opus 4.7、Claude Opus 4.5、Claude Sonnet 4、Claude Sonnet 3.7、Claude Haiku 4.5、Claude Haiku 3.5 等。注意 Anthropic 的 cache write 价格通常等于 input 价格，cache read 通常为 input 的 10%。fast mode 通常为 6x。200k token 以上有阶梯价
   - **OpenAI**: GPT-5 系列、o3/o4-mini、GPT-4.1 系列等。OpenAI 的 prompt caching 有 50% 折扣（cache read 为 input 的一半），cache write 和 cache read 同价。无阶梯定价（200k context 默认包含）
   - **Google**: Gemini 系列等。注意 cache read 通常有折扣。200k 以上有阶梯价
   - **DeepSeek**: DeepSeek-V3、DeepSeek-R1 等
   - **其他通过 OpenRouter 常用的模型**
2. 搜索每种模型在 **Claude Code / Codex** 日志中的实际模型名字符串，填入 `aliases`。常见变体：
   - 带/不带日期后缀（如 `claude-sonnet-4` vs `claude-sonnet-4-20250514`）
   - 带/不带 codex 后缀（如 `gpt-5` vs `gpt-5-codex`）
   - 带 provider 前缀（如 `anthropic/claude-sonnet-4-20250514` — 目前不需要，但如果发现日志中出现就需要）

### 当前已有内容（10 个模型，仅作起点参考）

当前文件 `price_catalog.json` 在 `crates/busytok-pricing/src/price_catalog.json`，包含：
- Anthropic: `claude-sonnet-4-20250514`
- OpenAI: `gpt-5`、`gpt-5.1-codex`、`gpt-5.2-codex`、`gpt-5.3-codex`、`gpt-5.4`、`gpt-5.4-mini`、`gpt-5.5`

你需要基于当前内容扩充为更完整的模型覆盖。

### 交付物

一份完整的 `price_catalog.json`，schema_version 为 `"3"`，包含上述格式规范的所有必填字段，version 和 updated 设为当天日期。
