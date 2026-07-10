# Busytok 打包发布流程

## 前置条件

- Git 仓库访问权限（BalianWang/busytok）
- GitHub CLI（`brew install gh` + `gh auth login`）
- macOS（如需本地构建调试）

## 速查清单

每一步的详细说明见下文：

- [ ] 1. `git checkout main && git pull origin main`
- [ ] 2. 编辑 `Cargo.toml` + `tauri.conf.json` 版本号
- [ ] 3. 提 PR（分支上 commit，**不打 tag**）→ 等 review 合入
- [ ] 4. 本地 `git pull main`，在 merge commit 上打 tag 并 push（触发 release CI）
- [ ] 5. 等 CI 完成（`gh run watch`）
- [ ] 6. `gh release edit vx.y.z --draft=false` 发布

---

## 1. 同步最新代码

```bash
cd /path/to/busytok
git checkout main
git pull origin main
git log --oneline -3
```

## 2. Bump 版本号

只在两个文件中改版本号（以 `0.0.10 → 0.0.11` 为例）：

**`apps/gui/src-tauri/Cargo.toml`** — 第 3 行：
```toml
version = "0.0.11"
```

**`apps/gui/src-tauri/tauri.conf.json`** — 第 4 行：
```json
"version": "0.0.11"
```

其他文件**不需要改**：
- `apps/cli/Cargo.toml` — CLI 有独立版本号，不跟发布走
- `Cargo.lock` — CI 构建时自动更新

改完确认无误：
```bash
git diff
```

## 3. 提 PR 合入 main

main 分支受保护，不能直接 push。通过 PR 合入：

```bash
# 创建分支 + 提交（不打 tag）
git checkout -b chore/release-0.0.11
git add apps/gui/src-tauri/Cargo.toml apps/gui/src-tauri/tauri.conf.json
git commit -m "chore(release): bump version 0.0.10 → 0.0.11"

# 推送分支（不推 tag）
git push origin chore/release-0.0.11

# 创建 PR
gh pr create \
  --base main \
  --head chore/release-0.0.11 \
  --title "chore(release): bump version 0.0.10 → 0.0.11" \
  --body "Release v0.0.11"
```

等 PR review 通过并 merge。

> **为什么不在 PR 阶段打 tag？** 如果在 PR 分支上打 tag 并 push，会立即触发 release workflow，但此时 tag 指向的 commit 还没合入 main。PR merge 后会产生新的 merge commit，tag 仍留在分支 commit 上，导致：
> 1. release CI 被触发两次（一次在分支 commit，一次如果移 tag 再 force push）
> 2. 需要额外的 `git tag -f` + `git push --force` 来把 tag 移到 merge commit
>
> 改为 merge 后在 main 上打 tag，release CI 只触发一次，tag 永远在 main 的 commit 上，无需 force push。

## 4. 在 merge commit 上打 tag（触发 release CI）

PR merge 后，main 上有了新的 merge commit。在 main 上打 tag 并推送，触发 release workflow：

```bash
git checkout main
git pull origin main
git tag v0.0.11
git push origin v0.0.11
```

验证 tag 在 main 上：
```bash
git merge-base --is-ancestor v0.0.11 origin/main && echo "OK" || echo "TAG NOT ON MAIN"
```

> **为什么不能直接在 main 上 commit + tag？** main 受保护，无法直接 push。只能从分支提 PR → merge。tag 在 merge 之后打，指向 main 的 merge commit。

## 5. 等待 CI 完成

CI 由 `v*` tag push 触发，流程约 15-20 分钟：

```
publish-macos:
  1. Install Rust + cross-compilation targets
  2. pnpm install
  3. cargo build aarch64 + x86_64, lipo 合并
  4. Tauri build .app
  5. npm install Pi SDK → .app bundle
  6. 下载 Node.js 22.23.1 双架构 → .app bundle
  7. 签名：helper 二进制 → node 二进制 → .node addons → bundle re-seal
  8. create-dmg → 签名 DMG
  9. Apple 公证 + staple
  10. 创建 draft GitHub Release + 上传产物（含自动生成的 release notes）
```

跟踪：
```bash
gh run list --limit=3 --workflow=release.yml
gh run watch <run-id>
```

## 6. 检查产物

```bash
gh release view v0.0.11 --json assets --jq '.assets[].name'
```

应有 6 个文件：
```
Busytok_0.0.11.dmg
Busytok.app.tar.gz
Busytok.app.tar.gz.sig
latest.json
versions.json
sha256sums.txt
```

## 7. 发布 Release

CI 创建的 release 是 **draft**（不会出现在 GitHub Releases 页面），手动发布：

```bash
gh release edit v0.0.11 --draft=false
```

CI 已通过 `softprops/action-gh-release` 的 `generate_release_notes: true`
自动从 PR 标题生成 release notes（changelog），无需手动编写。

验证：
```bash
gh release list --limit=3
# v0.0.11  Latest  ...
```

---

## 常见问题

### CI 编译/打包失败

```bash
gh run view <run-id> --log-failed
```

### 公证失败（Apple notarization）

错误关键字：`The binary is not signed` / `does not include a secure timestamp`

原因：bundle 内有未签名的 Mach-O 二进制。PR #78 已覆盖 `pi-sidecar/node_modules/**/*.node`。如出现新的未签名文件，在 `packaging/macos/scripts/_bundle_sidecar.sh` 的 `sign_sidecar_node_binaries()` 末尾追加签名逻辑。

### CI 没触发

```bash
# 检查 tag 是否在 origin/main 的祖先链上
git merge-base --is-ancestor v0.0.11 origin/main || echo "NOT ON MAIN"

# 如果不在，说明 tag 不是在 merge 后打的，重做 Step 4
```

### 本地构建 DMG（调试用，不需要 CI）

```bash
bash packaging/macos/scripts/package_dmg.sh
```

前置条件：macOS、Xcode、`brew install create-dmg`、Node.js、pnpm。
如未设置 `DEVELOPER_ID_APPLICATION` 环境变量，跳过签名（DMG 可用于本地测试但不能公证）。

产物：`target/universal-apple-darwin/release/bundle/dmg/Busytok_x.y.z.dmg`

### `gh` 命令不可用

```bash
brew install gh
gh auth login
```

---

## 版本号规范

| 内容 | 位置 | 示例 |
|------|------|------|
| 发布版本 | `apps/gui/src-tauri/Cargo.toml` `version` | `0.0.11` |
| 发布版本 | `apps/gui/src-tauri/tauri.conf.json` `version` | `0.0.11` |
| Git tag | — | `v0.0.11` |
| CLI 版本 | `apps/cli/Cargo.toml` `version` | `0.1.0`（独立） |
| Sidecar Node | `packaging/macos/scripts/_release_vars.sh` `SIDECAR_NODE_VERSION` | `22.23.1`（手动 bump） |

---

## 涉及文件

```
apps/gui/src-tauri/Cargo.toml                 ← bump 版本
apps/gui/src-tauri/tauri.conf.json            ← bump 版本
.github/workflows/release.yml                 ← CI（一般不用改）
.github/workflows/verify.yml                   ← CI（含打包脚本 smoke check）
packaging/macos/scripts/package_dmg.sh        ← 本地打包入口
packaging/macos/scripts/_bundle_sidecar.sh    ← Pi SDK + Node 注入
packaging/macos/scripts/_release_vars.sh      ← SIDECAR_NODE_VERSION
```
