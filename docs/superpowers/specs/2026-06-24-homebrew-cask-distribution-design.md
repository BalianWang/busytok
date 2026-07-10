# Homebrew Cask Distribution

- **状态**: Design approved
- **日期**: 2026-06-24
- **范围**: 将 Busytok 提交到官方 `homebrew/homebrew-cask` 仓库，使 macOS 用户可通过 `brew install --cask busytok` 安装。CI 自动提 bump PR。不涉及自有 tap、不涉及 Linux/Windows 包管理器。

---

## 1. 目标

用户 `brew install --cask busytok` 即可完成首次安装。后续更新由 Tauri 内置 updater 接管，brew 不参与版本管理。

---

## 2. 分发模型

```
brew install --cask busytok
       │
       ▼
  下载 DMG → 挂载 → 复制 Busytok.app 到 /Applications
       │
       ▼
  GUI 首次启动 → 写入 LaunchAgent plist → bootstrap service
       │
       ▼
  Tauri updater 检测新版本 → badge → 用户点击 → 下载+安装+重启
       │
       ▼
  brew upgrade 跳过此 cask（auto_updates true）
```

**关键决策**:

- **D1 — `auto_updates true`**. Tauri updater 是面向大多数用户的常规升级路径。Homebrew 负责首次安装；`brew outdated` 不会报告此 cask（不会在 `brew outdated` 列表中反复出现），但 `brew upgrade` 在没有设置 `HOMEBREW_NO_UPGRADE_AUTO_UPDATES_CASKS=1` 时仍可能拉取新版本。CI 同步 bump 公式（D3），确保不管用户走哪条路径都能拿到当前版本。这是 Cursor / Warp / Zed / Arc 的标准做法。
- **D2 — 官方 `homebrew/homebrew-cask` 仓库**. 不建自有 tap。首次提交需 community review（1-3 天），后续 bump PR 自动合入（通常几小时内）。
- **D3 — CI 自动 bump**. 每次 `release.yml` 发布新版本后，自动向 `homebrew/homebrew-cask` 提 PR，更新 `version` 和 `sha256`。人工无需介入。
- **D4 — `zap trash` 覆盖 Busytok 明确管理的固定路径**. 不猜测用户自定义路径（如 shim 脚本的 `--bin-dir`）。

---

## 3. Cask 公式

```ruby
cask "busytok" do
  version "0.0.11"
  sha256 "<由 CI 填入实际 DMG sha256>"

  url "https://github.com/BalianWang/busytok/releases/download/v#{version}/Busytok_#{version}.dmg"
  name "Busytok"
  desc "Local-first AI agent token usage audit dashboard"
  homepage "https://github.com/BalianWang/busytok"

  auto_updates true

  depends_on macos: ">= :sonoma"   # macOS 14.0

  app "Busytok.app"

  uninstall quit: "com.busytok.gui",
            launchctl: "com.busytok.service"

  zap trash: [
    "~/Library/Application Support/busytok/",
    "~/Library/LaunchAgents/com.busytok.service.plist",
  ]
end
```

### 3.1 字段说明

| 字段 | 说明 |
|------|------|
| `auto_updates true` | 告诉 brew：应用自己管理更新。`brew outdated` 不报告此 cask；`brew upgrade` 默认仍可能拉取新版本（除非设置 `HOMEBREW_NO_UPGRADE_AUTO_UPDATES_CASKS=1`）。 |
| `depends_on macos: ">= :sonoma"` | 对应 `tauri.conf.json` 的 `minimumSystemVersion: 14.0`。Homebrew 用 macOS 代号：`:sonoma` = 14.x。 |
| `uninstall quit` | `brew uninstall` 时先退出 `com.busytok.gui`（GUI 应用）。 |
| `uninstall launchctl` | 从 launchd 卸载 `com.busytok.service` 服务。 |
| `zap trash` | `brew uninstall --zap` 时删除的数据目录。覆盖 Busytok 明确管理的固定路径。 |

### 3.2 `zap trash` 覆盖范围

| 路径 | 内容 | 依据 |
|------|------|------|
| `~/Library/Application Support/busytok/` | SQLite 数据库（含 WAL）、`settings.toml`、`desktop_lifecycle.toml`、`price-catalog.json`、`service.ready`、`logs/`（GUI/service/CLI 日志及轮转）、`busytok-shim/`（shim 配置） | `BusytokPaths::data_dir()` → `~/Library/Application Support/busytok/` |
| `~/Library/LaunchAgents/com.busytok.service.plist` | GUI 运行时写入的 user-domain LaunchAgent | `managed_launch_agent.rs` |

### 3.3 不覆盖的路径

| 路径 | 原因 |
|------|------|
| `~/.local/bin/busytok`（或自定义 `--bin-dir`） | 用户安装 shim 时自定义路径，Cask 无法可靠反推。shim 是独立操作，不是 brew install 的副作用。 |
| `~/Library/Caches/` | 无 repo 代码写入此目录。 |
| `~/Library/Preferences/` | 无 repo 代码写入此目录。GUI 偏好使用 WebView `localStorage`（Tauri/WebKit 内部管理）。 |
| macOS AppKit saved-state | 系统自动管理，不纳入应用级清理。 |

---

## 4. CI 集成

### 4.1 前置准备（一次性）

1. Fork `https://github.com/Homebrew/homebrew-cask` 到 `BalianWang/homebrew-cask`
2. 创建 GitHub Personal Access Token (classic)，scope: `public_repo`，存入 repo secrets `HOMEBREW_CASK_TOKEN`
3. 设置 `HOMEBREW_GITHUB_API_TOKEN` 环境变量指向该 token
4. 手动提交初始公式到 fork → 向 upstream 提初始 PR。等待合入。

### 4.2 自动 bump 步骤

在 `release.yml` 的 `publish-macos` job 之后新增 `bump-homebrew-cask` job：

```yaml
  bump-homebrew-cask:
    needs: publish-macos
    runs-on: macos-latest
    if: startsWith(github.ref, 'refs/tags/v')  # 仅正式 tag
    environment: release
    steps:
      - uses: actions/checkout@v7

      - name: Extract version
        run: |
          APP_VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' apps/gui/src-tauri/Cargo.toml | head -1)
          echo "APP_VERSION=$APP_VERSION" >> $GITHUB_ENV

      - name: Bump homebrew-cask formula
        env:
          HOMEBREW_GITHUB_API_TOKEN: ${{ secrets.HOMEBREW_CASK_TOKEN }}
        run: |
          brew tap homebrew/cask
          brew bump-cask-pr busytok \
            --version "$APP_VERSION" \
            --commit-url "https://github.com/BalianWang/busytok/releases/tag/v$APP_VERSION"
```

`brew bump-cask-pr` 自动完成：
1. 下载对应版本 DMG，计算 SHA256
2. 更新 fork 中的 `Casks/b/busytok.rb`
3. 向 `homebrew/homebrew-cask` 提 PR，标题 `busytok <version>`
4. PR body 包含版本号和 commit URL

> **注意**: 此 job 仅在初始 cask PR 被 upstream 接受后才可用。在配置 `HOMEBREW_CASK_TOKEN` secret 之前，job 会因缺少 token 而跳过（不会阻塞 release）。

### 4.3 bump 频率

- **每 tag 自动提 PR**. `brew bump-cask-pr` 在已有 PR 未合入时不会重复提——它会检测到版本已最新。
- **首次提交（手动）**: 在 fork 中创建 `Casks/b/busytok.rb`，运行 `brew audit --cask busytok` 和 `brew style Casks/b/busytok.rb` 通过后，提 PR 到 upstream。

---

## 5. README 更新

在 `README.md` 的安装章节增加 Homebrew 路径：

```markdown
## Install (macOS)

### Homebrew (recommended)

brew install --cask busytok

Auto-updates are handled by the app's built-in updater.

### Manual download

Download the latest universal DMG from
[Releases](https://github.com/BalianWang/busytok/releases/latest) and drag
`Busytok.app` to `/Applications`.

**Apple Silicon and Intel are both supported** by the universal binary.
```

---

## 6. 验证标准

1. `brew install --cask busytok` 成功安装，`.app` 位于 `/Applications`
2. GUI 首次启动后，LaunchAgent plist 写入 `~/Library/LaunchAgents/`，服务启动
3. `brew outdated` 不报告 busytok（`auto_updates true` 生效）
4. `brew upgrade --cask busytok` 不会覆盖已通过 Tauri updater 更新的版本
5. `brew uninstall --cask busytok` 停止服务 + 删除 `.app`
6. `brew uninstall --zap --cask busytok` 额外删除 `~/Library/Application Support/busytok/` 和 `~/Library/LaunchAgents/com.busytok.service.plist`
7. CI tag push 后自动向 `homebrew/homebrew-cask` 提 bump PR
8. `brew audit --cask busytok` 通过
9. `brew style Casks/b/busytok.rb` 通过

---

## 7. 不在范围内

- 自有 tap（`BalianWang/homebrew-busytok`）
- Linux Homebrew（Linux 包管理器走 deb/rpm/AppImage，不在此次范围）
- Windows 包管理器（winget/chocolatey）
- CLI shim 的安装/卸载集成（`busytok shim install` 是独立操作，路径不可知，不纳入 cask zap）
