# Homebrew Cask Distribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Submit Busytok to the official `homebrew/homebrew-cask` repository so macOS users can `brew install --cask busytok`. Brew does first install; the built-in Tauri updater is the primary upgrade path.

**Architecture:** One cask formula (`Casks/b/busytok.rb`) in the official homebrew-cask repo, with `auto_updates true`. A `bump-homebrew-cask` CI job in `release.yml` automatically opens a bump PR on each tagged release. README updated to list Homebrew as the recommended install method.

**Tech Stack:** Ruby (cask DSL), GitHub Actions, `brew bump-cask-pr`, `brew audit`, `brew style`.

## Global Constraints

- **Formula location:** `Casks/b/busytok.rb` in `homebrew/homebrew-cask` (official repo, not a personal tap). Spec D2.
- **`auto_updates true`:** Tauri updater is the primary upgrade path. `brew outdated` skips this cask. `brew upgrade` may still pull new versions without `HOMEBREW_NO_UPGRADE_AUTO_UPDATES_CASKS=1`. CI bumps the formula on every release for users on the brew upgrade path. Spec D1.
- **`depends_on macos: ">= :sonoma"`** — matches `tauri.conf.json` `minimumSystemVersion: 14.0`. Homebrew codename: `:sonoma` = macOS 14.x.
- **`uninstall quit: "com.busytok.gui"`** — the app bundle ID from `tauri.conf.json:5`. **Not** `com.busytok.service` (that's the LaunchAgent label, used in `launchctl:`). Spec.
- **`zap trash`:** `~/Library/Application Support/busytok/` + `~/Library/LaunchAgents/com.busytok.service.plist`. No shim script paths (user-customizable, not a brew install side effect). Spec D4.
- **CI bump:** uses `brew bump-cask-pr` from the `homebrew-cask` tap. Requires a GitHub PAT with `public_repo` scope stored as `HOMEBREW_CASK_TOKEN` in repo secrets.
- **Manual first submission:** the initial formula must pass `brew audit --cask busytok` and `brew style Casks/b/busytok.rb` before the initial PR is opened. Subsequent bumps are automated.

---

## File Structure

- `docs/superpowers/specs/2026-06-24-homebrew-cask-distribution-design.md` — authoritative spec (already committed)
- `.github/workflows/release.yml` — add `bump-homebrew-cask` job
- `README.md` — update install section

**External repo (forked once, then CI-driven):**
- `BalianWang/homebrew-cask` — fork of `Homebrew/homebrew-cask`
- `Casks/b/busytok.rb` — the cask formula (in fork, PR'd to upstream)

---

## Task 1: Fork `homebrew-cask` and create the initial formula

**Files (in the fork):**
- Create: `Casks/b/busytok.rb`

**Interfaces:**
- Produces: a cask formula at `Casks/b/busytok.rb` in the fork, passing `brew audit` and `brew style`.

- [ ] **Step 1: Fork the upstream repo**

Go to `https://github.com/Homebrew/homebrew-cask` → Fork → `BalianWang/homebrew-cask`. Clone the fork locally.

- [ ] **Step 2: Write the cask formula**

Create `Casks/b/busytok.rb`:

```ruby
cask "busytok" do
  version "0.1.0-rc.7"  # from apps/gui/src-tauri/Cargo.toml — update on each release
  sha256 "REPLACE_WITH_ACTUAL_SHA256"

  url "https://github.com/BalianWang/busytok/releases/download/v#{version}/Busytok_#{version}.dmg"
  name "Busytok"
  desc "Local-first AI agent token usage audit dashboard"
  homepage "https://github.com/BalianWang/busytok"

  auto_updates true

  depends_on macos: ">= :sonoma"

  app "Busytok.app"

  uninstall quit: "com.busytok.gui",
            launchctl: "com.busytok.service"

  zap trash: [
    "~/Library/Application Support/busytok/",
    "~/Library/LaunchAgents/com.busytok.service.plist",
  ]
end
```

Compute the SHA256 from the latest release DMG (replace `<version>` with the
current version from `apps/gui/src-tauri/Cargo.toml`):

```bash
V=$(sed -n 's/^version = "\(.*\)"/\1/p' apps/gui/src-tauri/Cargo.toml | head -1)
curl -fsSL "https://github.com/BalianWang/busytok/releases/download/v${V}/Busytok_${V}.dmg" | shasum -a 256 | awk '{print $1}'
```

Replace `REPLACE_WITH_ACTUAL_SHA256` with the output.

- [ ] **Step 3: Audit + style check**

```bash
brew audit --cask Casks/b/busytok.rb
brew style Casks/b/busytok.rb
```

Expected: both pass with zero errors.

- [ ] **Step 4: Commit in the fork and open PR to upstream**

```bash
cd <homebrew-cask-fork>
git checkout -b add-busytok-cask
git add Casks/b/busytok.rb
git commit -m "busytok $(sed -n 's/^version = \"\(.*\)\"/\1/p' apps/gui/src-tauri/Cargo.toml | head -1) (new cask)"
git push origin add-busytok-cask
gh pr create --repo Homebrew/homebrew-cask --base main --head BalianWang:add-busytok-cask \
  --title "busytok $(sed -n 's/^version = \"\(.*\)\"/\1/p' apps/gui/src-tauri/Cargo.toml | head -1) (new cask)" \
  --body "Local-first AI agent token usage audit dashboard.
  macOS universal binary (Apple Silicon + Intel).
  Built-in auto-updater via Tauri updater (auto_updates true).
  Signed + notarized."
```

- [ ] **Step 5: Wait for upstream review**

The PR must pass Homebrew CI and get a maintainer approval (typically 1-3 days). Once merged, `brew install --cask busytok` works for all users.

---

## Task 2: Add `HOMEBREW_CASK_TOKEN` to repo secrets

**Files:**
- No code changes. GitHub repo settings.

**Interfaces:**
- Produces: a secret `HOMEBREW_CASK_TOKEN` available to `release.yml` CI jobs.

- [ ] **Step 1: Create a GitHub Personal Access Token (classic)**

Go to `https://github.com/settings/tokens` → Generate new token (classic).
- Note: `busytok homebrew-cask bump`
- Expiration: `No expiration` (or 1 year + calendar reminder)
- Scope: `public_repo`
- Copy the token.

- [ ] **Step 2: Add to repo secrets**

Go to `https://github.com/BalianWang/busytok/settings/secrets/actions` → New repository secret.
- Name: `HOMEBREW_CASK_TOKEN`
- Value: `<paste the token>`

---

## Task 3: Add `bump-homebrew-cask` job to `release.yml`

**Files:**
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `HOMEBREW_CASK_TOKEN` secret (Task 2).
- Produces: automatic bump PR to `homebrew/homebrew-cask` on every tagged release.

- [ ] **Step 1: Write the bump job**

Append to `.github/workflows/release.yml`, after the existing `publish-macos` job:

```yaml
  bump-homebrew-cask:
    needs: publish-macos
    runs-on: macos-latest
    if: startsWith(github.ref, 'refs/tags/v')
    environment: release
    steps:
      - uses: actions/checkout@v7

      - name: Extract app version from Cargo.toml
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
            --message "Automated bump by Busytok release CI.
            
            https://github.com/BalianWang/busytok/releases/tag/v$APP_VERSION" \
            --fork-org BalianWang
```

The `brew bump-cask-pr` command:
1. Downloads the DMG for `$APP_VERSION`, computes SHA256.
2. Updates `Casks/b/busytok.rb` in the `BalianWang/homebrew-cask` fork.
3. Opens a PR against `Homebrew/homebrew-cask` with title `busytok <version>`.

- [ ] **Step 2: Verify the YAML**

```bash
yamllint .github/workflows/release.yml   # if yamllint is available
# Or just: confirm GitHub Actions can parse it on next push
```

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(release): add bump-homebrew-cask job for automatic formula bumps

Uses brew bump-cask-pr to open a PR against homebrew/homebrew-cask
on each tagged release. Requires HOMEBREW_CASK_TOKEN secret."
```

---

## Task 4: Update `README.md` with Homebrew install instructions

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Rewrite the Install section**

Replace the existing `## Install (macOS)` section (lines ~13-19):

```markdown
## Install (macOS)

### Homebrew (recommended)

brew install --cask busytok

Updates are handled by the built-in auto-updater. You can also
`brew upgrade --cask busytok` to pull the latest version via Homebrew.

### Manual download

Download the latest universal DMG from
[Releases](https://github.com/BalianWang/busytok/releases/latest) and drag
`Busytok.app` to `/Applications`.

**Apple Silicon and Intel are both supported** by the universal binary.
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add Homebrew install to README

brew install --cask busytok as recommended method."
```

---

## Task 5: End-to-end verification

**Files:**
- No new files. Manual verification only.

- [ ] **Step 1: Fresh install via Homebrew**

On a clean machine (or after `brew uninstall --zap --cask busytok`):

```bash
brew install --cask busytok
```

Expected: DMG downloaded, `Busytok.app` in `/Applications`. Launch GUI → service starts.

- [ ] **Step 2: Verify `brew outdated` skips the cask**

```bash
brew outdated --cask
```

Expected: busytok NOT listed.

- [ ] **Step 3: Verify uninstall**

```bash
brew uninstall --cask busytok
```

Expected: GUI quit, LaunchAgent unloaded, `.app` removed from `/Applications`.

- [ ] **Step 4: Verify zap cleanup**

```bash
brew uninstall --zap --cask busytok
```

Expected after zap: `~/Library/Application Support/busytok/` removed, `~/Library/LaunchAgents/com.busytok.service.plist` removed.

- [ ] **Step 5: Verify CI bump on next release**

Push a tag for the next release. After `publish-macos` completes, check that the `bump-homebrew-cask` job runs and a PR appears at `https://github.com/Homebrew/homebrew-cask/pulls`.

> **Note on AC4 (upgrade non-downgrade):** Spec AC4 requires verifying `brew upgrade --cask busytok` does not downgrade an app already updated via the Tauri updater. This requires at least two releases. Deferred to the second release cycle: after the first CI bump PR is merged, locally update via the Tauri updater to a newer version, then run `brew upgrade --cask busytok` and confirm it does not replace the newer `.app`.

---

## Verification gate

- [ ] `brew audit --cask Casks/b/busytok.rb` — clean
- [ ] `brew style Casks/b/busytok.rb` — clean
- [ ] `brew install --cask busytok` — installs correctly
- [ ] `brew uninstall --zap --cask busytok` — cleans up data paths
- [ ] CI `bump-homebrew-cask` job succeeds on first tagged release after merge

---

## Self-Review

**1. Spec coverage:**

| Spec requirement | Task |
|---|---|
| D1 `auto_updates true` formula | Task 1 |
| D2 Official `homebrew/homebrew-cask` | Task 1 (PR to upstream) |
| D3 CI auto bump | Tasks 2 + 3 |
| D4 `zap trash` paths | Task 1 |
| Cask formula (§3) | Task 1 |
| CI integration (§4) | Tasks 2 + 3 |
| README update (§5) | Task 4 |
| Verification (§6, AC 1-9) | Task 5 |

**2. Placeholder scan:** No TBD/TODO. SHA256 in Task 1 Step 2 is explicitly "computed from the actual DMG" with the curl command provided. The version number in Task 1 is sourced from `apps/gui/src-tauri/Cargo.toml` and should match the current release tag.

**3. Type consistency:** Single-subsystem plan — no cross-task type conflicts. The `APP_VERSION` extraction in Task 3 uses the same `sed` pattern as Task 1 and the existing `release.yml` steps.
