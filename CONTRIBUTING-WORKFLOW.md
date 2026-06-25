# CI & PR Workflow — Busytok

> 交接文档。本文描述从分支创建到 release 出包的完整 CI/PR 流程与规范。

## 仓库信息

| 仓库 | 可见性 | 分支 | 用途 |
|------|--------|------|------|
| `BalianWang/busytok` | **PUBLIC** | `main`（唯一长期分支） | 开源发布 |
| `BalianWang/autoken` | PRIVATE | `dev` | 内部开发镜像 |

公开仓库 `main` 受保护，禁止直推。所有变更必须通过 **PR → CI 绿 → rebase merge**。

## Branch Protection（`main`）

| 规则 | 值 |
|------|-----|
| Required status checks | `verify (ubuntu-latest)`, `verify (macos-latest)`, `verify (windows-latest)` |
| Required PR before merge | yes（approval count = 0） |
| Required linear history | yes（强制 rebase merge，禁止 merge commit） |
| Force push / deletion | blocked |
| Enforce admins | yes |

## verify.yml — CI 门禁

**触发条件**：`push` 到 `main`，`pull_request` 到 `main`

**矩阵**：三平台 `ubuntu-latest` / `macos-latest` / `windows-latest`，`fail-fast: false`

**实际执行步骤**（5 个强制门禁）：

| 步骤 | 平台 | 说明 |
|------|------|------|
| `cargo fmt --check` | 全平台 | Rust 格式化检查 |
| `cargo clippy --workspace -- -D warnings` | 全平台（GUI crate 仅 macOS 包含） | Lint 强制零警告 |
| `cargo test --workspace` | 全平台（GUI crate 仅 macOS 包含） | 单元 + 集成测试 |
| `cargo audit` | ubuntu only | 安全公告检查 |
| `pnpm typecheck` | 全平台 | TypeScript 类型检查 |

**推迟项**（`verify.yml` 中以注释记录，待后续修复后重新启用）：

| 推迟项 | 原因 |
|--------|------|
| `cargo deny check` | 需逐 crate license 调整 |
| `cargo llvm-cov` | Instrumented tests 在 CI runner 上崩溃；`COVERAGE_GATE` 可通过 repo variable 设置（默认 80%）。本地运行：`bash scripts/coverage.sh` |
| `pnpm test` / `pnpm test:coverage` | Vitest 在 CI runner 上挂起 |
| Release workflow smoke test | 同上 |

**关键配置**：

- `concurrency: cancel-in-progress: true` — 同一分支/PR 新 push 自动取消队列中的旧 run。
- `permissions: contents: read` — 最小权限声明，防御纵深。

## release.yml — 发布管道

**触发条件**：`push: tags: ['v*']`（仅 tag 触发，PR 和分支 push 不触发）

**发布步骤**（macOS-only）：

1. Rust cross-compile（aarch64 + x86_64）→ `lipo` 合并 universal binary
2. Tauri bundle（`.app`）
3. Helper 注入 + 双层签名（helper binary + bundle 整体）
4. `create-dmg` 创建 DMG
5. `xcrun notarytool` 公证 + `stapler staple`
6. Updater payload 生成（`.tar.gz` + `.sig`）
7. `latest.json` 生成
8. Draft GitHub Release + 上传资产

**Secrets**（全部在 `release` Environment）：

| Secret | 用途 |
|--------|------|
| `APPLE_CERTIFICATE` | Developer ID .p12（base64） |
| `APPLE_CERTIFICATE_PASSWORD` | .p12 密码 |
| `APPLE_SIGNING_IDENTITY` | `"Developer ID Application: ..."` |
| `APPLE_API_KEY` | App Store Connect API key（.p8 原文） |
| `APPLE_API_KEY_ID` | Key ID |
| `APPLE_API_ISSUER` | Issuer UUID |
| `TAURI_SIGNING_PRIVATE_KEY` | Updater 签名私钥 |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 私钥密码 |

**Environment 保护**：

- `release` environment 已开启 **Required reviewers**（reviewer = `BalianWang`）。
- 每次 tag push 触发 release 后，workflow 进入 `waiting` 状态，需 reviewer 在 Web UI 上点击 **Approve and deploy** 才能继续。
- Release 产出为 **draft**，不自动 publish。

**Release 流程**：

```bash
# 1. bump 版本号 (Cargo.toml + tauri.conf.json)
# 2. 走正常 PR 流程合并到 main
# 3. 打 tag 并推送
git tag v0.1.0-rc.N
git push origin v0.1.0-rc.N

# 4. 到 https://github.com/BalianWang/busytok/actions 找到 release workflow run
# 5. 点击 Approve and deploy
# 6. 约 8–15 分钟后 draft release 生成，验收 5 个资产：
#    - Busytok_x.y.z.dmg
#    - Busytok.app.tar.gz
#    - Busytok.app.tar.gz.sig
#    - latest.json
#    - sha256sums.txt
```

## 标准 PR 流程

```bash
# 1. 从 main 最新版本创建分支
git checkout main
git pull origin main --ff-only
git checkout -b <category>/<short-description>

# 分支命名示例：
#   fix/deepseek-cache-read-double-count
#   feat/add-sidechain-dedup
#   refactor/consolidate-write-paths

# 2. 本地验证（PR 提交前必须跑）
cargo fmt --all --check
cargo clippy --workspace --exclude busytok-gui --all-targets -- -D warnings
cargo test --workspace --exclude busytok-gui
pnpm typecheck

# 3. 推送分支
git push -u origin <branch-name>

# 4. 创建 PR（建议用 gh CLI）
gh pr create \
  --base main \
  --head <branch-name> \
  --title "type(scope): short description" \
  --body "## Summary

  What and why.

  ## Changes

  - Specific change 1
  - Specific change 2

  ## Test plan

  How verified."
```

## PR 规范

**标题（Conventional Commits）**：

```
feat(scope): description      新功能
fix(scope): description       修复
refactor(scope): description  重构
docs(scope): description      文档
test(scope): description      测试
ci: description               CI/CD
chore(scope): description     杂项
```

**Description 模板**（`.github/PULL_REQUEST_TEMPLATE.md` 自动填充）：
- Summary
- Linked issue / discussion
- Checklist

**合并策略**：**rebase merge**，删除源分支。

## 本地验证

```bash
# 快速验收门禁（推荐每次提交前跑）
./scripts/verify_acceptance.sh

# 完整发布 rehearsal（慢，仅 macOS）
./scripts/verify_release.sh

# 命名检查
bash scripts/check-busytok-naming.sh

# 覆盖率（本地）
bash scripts/coverage.sh        # 默认 80% gate
COVERAGE_GATE=85 bash scripts/coverage.sh   # 目标门禁
```

## 常见问题

**Q: PR CI 失败怎么办？**
1. 点进 GitHub Actions run 看具体失败日志
2. 本地修复 → `git commit --amend`（如还在 review）或追加 commit → `git push`

**Q: 两个分支改了同一个文件有冲突？**
```bash
# 本地试合
git checkout <your-branch>
git merge main --no-commit --no-ff
# 解决冲突 → git add <file> → git merge --continue
# 或放弃 → git merge --abort
```

**Q: 推送被拒（GH006: Protected branch）？**
这意味着你在尝试直推 `main`。走 PR 流程。

**Q: 本地数据库需要重置？**
```bash
rm -f ~/Library/Application\ Support/Busytok/*.db
# schema migration 会在下次启动自动执行
```
