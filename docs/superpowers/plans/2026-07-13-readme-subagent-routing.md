# Subagent-Focused README Rewrite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite the repository README around Busytok's task-level model routing through logical subagents, with an English canonical version and a Chinese mirror.

**Architecture:** Keep the README as a concise product entry point. Put the shortest trustworthy path (install, readiness checks, one synchronous delegation, one asynchronous polling flow) in both README files, and link detailed integration/testing/development material instead of duplicating it. `README.md` owns the English wording; `README.zh-CN.md` mirrors its headings and executable examples.

**Tech Stack:** Markdown, GitHub-relative links/images, Busytok CLI examples, existing repository documentation.

---

### Task 1: Map source-of-truth documentation and executable examples

**Files:**
- Read: `docs/superpowers/guides/busytok-subagent-codex-integration.md`
- Read: `docs/subagent-testing-guide.md`
- Read: `CONTRIBUTING.md`, `SECURITY.md`, `docs/release-workflow.md`
- Read: `apps/cli/src/main.rs` and relevant CLI command modules for help/flag names

- [ ] **Step 1: Verify the public command contract**

Check the exact forms of `busytok status`, `busytok models --json`, `busytok delegate`, `--reuse-policy`, `--wait`, `--wait-timeout`, `--output json`, and `busytok subagent task --task-id` against the integration guide and CLI source.

- [ ] **Step 2: Record documentation links and product boundaries**

Use only existing paths and claims supported by the current implementation. Preserve the DMG installation and 0.x stability guidance; do not introduce Homebrew Cask instructions or unsupported transparent-proxy claims.

- [ ] **Step 3: Check the current README assets**

Confirm the dashboard and prompt-palette screenshots exist before deciding whether both remain in the product-focused README.

Run: `test -f docs/assets/dashboard.png && test -f docs/assets/prompt-palette.png`

Expected: exit code `0`.

### Task 2: Rewrite the canonical English README

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Replace the opening positioning**

Lead with explicit task-level model routing through persistent logical subagents. Explain that a caller chooses a provider/model binding per logical identity and delegates a task to it.

- [ ] **Step 2: Add a concise capability section**

Cover explicit provider/model binding, stable identities, `create`/`reuse`/`fail`, async polling, task history, cancellation, per-subagent serialization, session reuse, queue reasons, and structured diagnostics without claiming unsupported automatic routing.

- [ ] **Step 3: Add the quick-start flow**

Include macOS DMG installation, `busytok status`, `busytok models --json`, and a provider/model placeholder followed by a real `busytok delegate ... --wait --output json` example. Tell readers to source IDs from the live catalog rather than assuming a fixed provider.

- [ ] **Step 4: Add the asynchronous orchestration example**

Show `busytok delegate --output json` returning a task ID and `busytok subagent task --task-id "<TASK_ID>" --output json` for polling. Mention that stdout is machine-readable and diagnostics remain on stderr.

- [ ] **Step 5: Explain core concepts and boundaries**

Document binding immutability under reuse, logical identity, queue/cancel/history behavior, local-first persistence, and the non-goals around transparent proxying, TLS interception, and external OAuth/API sessions.

- [ ] **Step 6: Reorganize links and preserve useful existing sections**

Add a documentation table, keep workspace/development verification, contributing, security, license, badges, installation, and stability contract. Link to `README.zh-CN.md` near the top.

### Task 3: Create the Chinese mirror

**Files:**
- Create: `README.zh-CN.md`

- [ ] **Step 1: Mirror the English heading structure**

Translate prose into Simplified Chinese while preserving section order, code blocks, CLI flags, JSON names, placeholders, links, badges, and image paths exactly.

- [ ] **Step 2: Translate product terminology consistently**

Keep `provider`, `model`, `logical subagent`, `binding`, `delegate`, `task`, `reuse policy`, and status values in code formatting; provide Chinese explanations without inventing alternate CLI terms.

- [ ] **Step 3: Add language navigation**

Link back to `README.md` at the top and ensure both language versions point to the same detailed documentation.

### Task 4: Validate bilingual parity and stale claims

**Files:**
- Test: `README.md`, `README.zh-CN.md`

- [ ] **Step 1: Compare heading outlines**

Run:

```bash
python3 - <<'PY'
from pathlib import Path
import re
def headings(path):
    return [re.sub(r'^#+\\s*', '', line).strip() for line in Path(path).read_text().splitlines() if line.startswith('#')]
en = headings('README.md')
zh = headings('README.zh-CN.md')
assert len(en) == len(zh), (len(en), len(zh))
print(f'heading counts match: {len(en)}')
PY
```

Expected: `heading counts match` with equal counts.

- [ ] **Step 2: Check executable command coverage**

Run: `rg -n "busytok (status|models --json|delegate|subagent task --task-id)" README.md README.zh-CN.md`

Expected: each required command appears in both files.

- [ ] **Step 3: Check stale or prohibited claims**

Run: `rg -n "does not .*route models|Homebrew|transparent proxy|TLS interception|OAuth" README.md README.zh-CN.md`

Expected: no stale “does not route models” or unsupported distribution claim; boundary language may mention transparent proxy/TLS/OAuth only as explicit non-goals.

- [ ] **Step 4: Validate links and assets**

Run: `git diff --check` and inspect all new relative links and image paths.

Expected: no whitespace errors and no link points to a file that does not exist.

### Task 5: Review and commit documentation changes

**Files:**
- Modify: `README.md`
- Create: `README.zh-CN.md`

- [ ] **Step 1: Review the final diff for factual drift**

Confirm that every command is supported by the integration guide/CLI, the live catalog is treated as authoritative, and the README remains concise enough for GitHub’s project front door.

- [ ] **Step 2: Run the repository documentation checks**

Run: `git diff --check` and the parity checks from Task 4.

Expected: all checks pass.

- [ ] **Step 3: Commit the README rewrite**

```bash
git add README.md README.zh-CN.md
git commit -m "docs: rewrite README around subagent routing"
```

