# Busytok

[![CI](https://github.com/BalianWang/busytok/actions/workflows/verify.yml/badge.svg?branch=main)](https://github.com/BalianWang/busytok/actions/workflows/verify.yml)
[![Release](https://img.shields.io/github/v/release/BalianWang/busytok?include_prereleases)](https://github.com/BalianWang/busytok/releases)
[![License: Apache--2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)

[English](README.md) | 简体中文

**Busytok 通过持久化的 logical subagent 身份，将委派任务路由到明确绑定的 provider/model。** 它采用本地优先模式：桌面应用和 `busytok` CLI 在本机协调任务执行、排队和诊断，同时保留现有的本地 agent 使用量元数据审计面板。

![Busytok Dashboard](docs/assets/dashboard.png)

## 为什么要进行任务级路由？

不同任务需要不同模型，而稳定的角色应持续使用同一个路由决策，直到你
明确更改它。Busytok 允许调用方为 logical subagent 选择 provider 和
model，委派一个 task，然后等待、轮询、取消或检查结果，并且不会悄悄地
重新绑定该角色。

## 功能

- 从实时 catalog 获取并明确绑定 provider/model
- 面向角色的持久化 logical subagent 身份
- `create`、`reuse`、`fail` reuse policy，且不会悄悄重新绑定
- 同步 `--wait` 完成，或异步 JSON 提交与轮询
- 单个 logical subagent 的任务串行化、排队和 sidecar 会话复用
- 排队原因、取消、任务历史和结构化错误诊断
- 本地 SQLite 持久化，以及用于本地 agent 元数据的桌面审计面板

## 快速开始（macOS）

### 1. 安装应用

从 [Releases](https://github.com/BalianWang/busytok/releases/latest) 下载最新的通用 DMG，打开后将 `Busytok.app` 拖到 `/Applications`。Apple Silicon 和 Intel 均受支持。

### 2. 检查服务和 catalog 是否就绪

启动应用，然后确认本地服务已就绪并查看启用的 provider/model catalog：

```bash
busytok status
busytok models --json
```

在 GUI 或 CLI 中至少配置一个 provider 和一个启用的 model。使用
`busytok models --json` 返回的 `provider_id` 和 `model_id`；不要假定固定
provider 或 catalog 顺序。

### 3. 委派并等待

将 `<PROVIDER_ID>` 和 `<MODEL_ID>` 替换为实时 catalog 中的 ID：

```bash
busytok delegate \
  --subagent "reviewer-001" \
  --profile "pi/review-cheap" \
  --bind-provider "<PROVIDER_ID>" \
  --bind-model "<MODEL_ID>" \
  --reuse-policy create \
  --output json \
  --wait \
  --wait-timeout 120 \
  "Review the repository's open TODOs and return the three highest-impact items."
```

响应会以机器可读 JSON 写入 stdout。请将 stderr 与之分开以获取诊断信息；
自动化时不要合并两个流。

## 异步委派

对于较长任务，不使用 `--wait` 提交，读取返回的 `task_id`，然后轮询任务，
直到状态结束：

```bash
busytok delegate \
  --subagent "reviewer-async-001" \
  --profile "pi/review-cheap" \
  --bind-provider "<PROVIDER_ID>" \
  --bind-model "<MODEL_ID>" \
  --reuse-policy create \
  --output json \
  "Review the repository's open TODOs and return the three highest-impact items."

busytok subagent task --task-id "<TASK_ID>" --output json
```

`completed` 表示成功；`queued` 和 `running` 表示仍在进行；`failed` 和
`cancelled` 应连同结构化错误或取消上下文一起报告。有关确定性 catalog
选择、prompt 渠道和取消流程，请参阅[集成指南](docs/superpowers/guides/busytok-subagent-codex-integration.md)。

## 核心概念

### Provider/model 绑定

每个 logical subagent 都可以绑定 provider UUID（`--bind-provider`）和
model ID（`--bind-model`）。`busytok models --json` 返回的实时 catalog 是
唯一事实来源。单任务的 `--model` 覆盖与持久化绑定不同。

### Logical subagent 与 reuse policy

`--subagent` 是稳定的、面向角色的身份。可使用：

- `--reuse-policy create` 创建新的路由身份
- `--reuse-policy reuse` 有意使用现有绑定
- `--reuse-policy fail` 在名称冲突时拒绝操作

复用名称永远不会悄悄重新绑定。若要将角色路由到其他 provider/model，
请创建新的 logical subagent 名称。

### 任务生命周期

运行时会串行执行同一 logical subagent 的工作，在需要时排队，并可复用其
sidecar 会话。提交后可使用任务轮询、取消和历史命令管理工作；结构化的
排队原因和错误有助于诊断失败。

## 产品边界

Busytok 采用本地优先模式，应用数据存储在本地 SQLite 中。Provider 和
model 由你在 GUI 或 CLI 中配置，委派操作是显式的。它不是面向所有
Claude/Codex 流量的 transparent proxy，不会执行 TLS interception，也不
管理外部 agent 的 OAuth/API sessions。它不承诺云托管、自动路由每个外部
agent 请求，也不承诺固定的 provider catalog。

## 本地桌面功能

对于导入的 Claude Code 和 Codex 使用量日志，桌面应用保存 token/使用量
元数据，而不是 prompt/response 正文。委派任务的 prompt/result 以及
prompt palette 模板属于单独的本地应用数据。桌面 UI 提供 Overview、Usage、
Prompt Palette、Providers、Subagents 和 Settings 视图。按 **`Cmd+Option+K`** 可打开可选的
prompt palette，用于保存和复用本地 prompt 模板。

![Prompt Palette](docs/assets/prompt-palette.png)

## 文档

| 主题 | 指南 |
| --- | --- |
| Agent 集成和 CLI 契约 | [Subagent 委派指南](docs/superpowers/guides/busytok-subagent-codex-integration.md) |
| Subagent 测试和隔离 | [Subagent 测试指南](docs/subagent-testing-guide.md) |
| 产品设计 | [Design](DESIGN.md) · [Design system](DESIGN-SYSTEM.md) |
| 发布 | [发布流程](docs/release-workflow.md) |
| 开发和贡献 | [贡献指南](CONTRIBUTING.md) |
| 安全报告 | [安全策略](SECURITY.md) |
| 许可证 | [Apache-2.0](LICENSE) |

## 工作区和验证

- `apps/gui`：React + Tauri 桌面应用
- `apps/gui/src-tauri`：Tauri Rust 主机 crate 和打包配置
- `apps/service`：Rust 后台服务
- `apps/cli`：Rust 管理 CLI
- `crates/busytok-*`：Rust workspace crate

在提交 pull request 前运行本地验收门禁：

```bash
./scripts/verify_acceptance.sh
```

在 macOS 上进行发布演练：

```bash
DEVELOPER_ID_APPLICATION="Developer ID Application: ..." ./scripts/verify_release.sh
```

命名检查命令：

```bash
bash scripts/check-busytok-naming.sh
```

## 稳定性契约

Busytok 处于 `0.x`：真实可用，但**次版本可能带来破坏性变更**。macOS
发布使用通用 DMG，并可能自动更新；需要时请从 [Releases](https://github.com/BalianWang/busytok/releases/latest) 手动重新安装。

## 贡献

工具链、分支模型和必需的 CI 检查见 [`CONTRIBUTING.md`](CONTRIBUTING.md)。
Pull request 应提交到 `main`，标题使用 Conventional Commits。

## 安全

请参阅 [`SECURITY.md`](SECURITY.md)。通过 [GitHub Private Vulnerability
Reporting](https://github.com/BalianWang/busytok/security/advisories/new) 报告
漏洞；安全报告不要公开提交 issue。

## 许可证

[Apache-2.0](LICENSE)
