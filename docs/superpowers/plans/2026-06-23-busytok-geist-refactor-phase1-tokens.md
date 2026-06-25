# Busytok Geist Refactor — Phase 1: Token & Material Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite the GUI's token + material contract (`tokens.css`) to a de-glassed, opaque, Geist-calibrated foundation and migrate every consumer, with zero component-structure changes — producing a calmer baseline that still renders correctly and is fully test/guard-covered.

**Architecture:** Token-first (spec Phase 1). All visual calming cascades from `tokens.css`; the three consumer stylesheets (`surfaces/components/pages.css`) are migrated mechanically to the new vocabulary. The contract is enforced three ways: (1) `tokens.test.ts` asserts literal token values, (2) `scripts/check-busytok-gui-surfaces.sh` (existing bash `rg` guard) is extended to forbid stale token names / stray `backdrop-filter` / radius outliers / raw hex, (3) the existing 90% coverage gate. A one-shot design-system telemetry marker is emitted at bootstrap via the existing `reporter.ts` so any field-reported visual regression can be correlated to this token layer.

**Tech Stack:** React 19 + Vite 7 + Vitest 3 (string-contract tests over `tokens.css`), plain CSS custom properties, Tauri 2, `reporter.ts` (`safeReportEvent`) for observability, bash `rg` for guards.

## Global Constraints

(From spec `docs/superpowers/specs/2026-06-22-busytok-geist-refactor-design.md`. Every task implicitly includes these.)

- **No component-structure changes this phase.** Token values change and cascade; component logic/JSX is untouched (except the one bootstrap observability hook in Task 7).
- **No dead code.** Removed/renamed tokens are deleted everywhere — no compatibility aliases, in CSS **and TS**. Must be gone after Phase 1: `surface-strong`, `surface-elevated`, `canvas-subtle`, `border-soft`, `sidebar`, `radius-xs`, `radius-xl`, plus the now-dead `--material-surface-alpha` / `--material-surface-strong-alpha` (zero consumers once the translucent tiers collapse).
- **Token contract is consumed by TS too.** `chartTokens.ts`, `nivoTheme.ts`, and `LiveCurvePanel.tsx` read CSS vars at runtime (`cssVar()` / `resolveCssColor` / inline `var()`); their token-string literals MUST be migrated alongside the CSS, or charts silently fall back to hardcoded colors and lose dark-theme adaptation.
- **Indigo accent + SF Pro font are unchanged** — only neutral/material/radius/data-tempering tokens change.
- **Coverage gate:** `pnpm coverage:gui` requires ≥90% lines. Token CSS edits do not lower TS coverage; every new TS module (Task 7) ships with a test.
- **Reuse existing infra:** extend `tokens.test.ts` and `scripts/check-busytok-gui-surfaces.sh` in place; do not introduce a new lint toolchain.
- **Observability** uses `safeReportEvent` from `apps/gui/src/logging/reporter.ts`; event codes follow the existing `gui.<area>.<event>` convention.
- **Commit per task.** Run `pnpm --filter @busytok/gui test` + `pnpm check:gui-surfaces` + `pnpm --filter @busytok/gui typecheck` green before each commit.

## File Structure

| File | Responsibility | This phase |
|---|---|---|
| `apps/gui/src/styles/tokens.css` | The contract — neutral/material/radius/data tokens, light + dark | Rewrite values, rename/remove tokens |
| `apps/gui/src/styles/tokens.test.ts` | String-contract assertions over `tokens.css` | Extend with new value/name assertions |
| `apps/gui/src/styles/surfaces.css` | Shell containers (`.desktop-shell`, `.page-surface`) | Migrate renames; drop `backdrop-filter` from `.page-surface` |
| `apps/gui/src/styles/components.css` | Sidebar/titlebar/chip/dialog/palette/table | Migrate renames + surface-tier repoint |
| `apps/gui/src/styles/pages.css` | Overview/metric/heatmap/ranking/tables | Migrate renames + surface-tier repoint + radius outliers |
| `apps/gui/src/lib/chartTokens.ts`, `nivoTheme.ts` | Chart theme via runtime CSS-var resolution | Migrate `--color-border-soft` / `--color-surface-strong` string literals |
| `apps/gui/src/components/overview/LiveCurvePanel.tsx` (+ `.test.tsx`) | Live chart grid/loading color via CSS vars | Migrate `--color-border-soft` / `--color-surface-strong` string literals |
| `scripts/check-busytok-gui-surfaces.sh` | bash `rg` regression guard | Add stale-token / blur / radius / hex rules (scan CSS **and TS**) |
| `apps/gui/src/logging/designSystem.ts` | **New** — design-system version marker | Created (Task 7) |
| `apps/gui/src/logging/designSystem.test.ts` | **New** — marker test | Created (Task 7) |
| `apps/gui/src/main.tsx` | Bootstrap | Call marker once after `initThemeRuntime()` (Task 7) |

**Remap rules (locked, used across Tasks 1–2):**
- `--color-surface-strong` → `--color-surface` (floating/strong bodies → primary opaque surface)
- `--color-surface-elevated` → `--color-surface-subtle` (raised/hover fills → subtle tier)
- `--color-canvas-subtle` → `--color-surface-subtle` (rename)
- `--color-border-soft` → `--color-border-subtle` (rename)
- `--color-sidebar` → `--color-chrome` (rename)

**TS consumers (P0):** the renames above also apply to the CSS-var string literals inside `chartTokens.ts`, `chartTokens.test.ts`, `nivoTheme.ts`, `LiveCurvePanel.tsx`, and `LiveCurvePanel.test.tsx` — these resolve tokens at runtime, so a missed migration silently breaks chart theming. Tasks 1 & 2 include them in the substitution loops; Task 8's guard scans all of `apps/gui/src` (not just CSS).

---

### Task 1: Token vocabulary rename (zero visual change)

**Files:**
- Modify: `apps/gui/src/styles/tokens.css`
- Modify: `apps/gui/src/styles/surfaces.css`, `apps/gui/src/styles/components.css`, `apps/gui/src/styles/pages.css`
- Test: `apps/gui/src/styles/tokens.test.ts`

**Interfaces:**
- Consumes: existing token values (unchanged this task).
- Produces: new public names `--color-border-subtle`, `--color-surface-subtle`, `--color-chrome`, `--color-hover`, `--color-hover-strong`. Old names (`--color-border-soft`, `--color-canvas-subtle`, `--color-sidebar`) removed. **Values are unchanged** — this is a pure rename so a reviewer can verify no pixel shifted; values change in later tasks.

- [ ] **Step 1: Write the failing contract test**

Append to the first `it(...)` block inside `apps/gui/src/styles/tokens.test.ts` (the one starting `it("defines the required public token API..."`), before its closing `});`:

```ts
    // Phase 1 rename: new vocabulary exists, old names are gone.
    expect(css).toContain("--color-border-subtle:");
    expect(css).toContain("--color-surface-subtle:");
    expect(css).toContain("--color-chrome:");
    expect(css).toContain("--color-hover:");
    expect(css).toContain("--color-hover-strong:");
    expect(css).not.toContain("--color-border-soft:");
    expect(css).not.toContain("--color-canvas-subtle:");
    expect(css).not.toContain("--color-sidebar:");
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm --filter @busytok/gui test -- src/styles/tokens.test.ts`
Expected: FAIL — `expect(css).toContain("--color-border-subtle:")` fails (token not yet defined).

- [ ] **Step 3: Rename tokens in `tokens.css`**

In `apps/gui/src/styles/tokens.css`, rename declarations (values untouched):

1. `--color-canvas-subtle:` → `--color-surface-subtle:` (light block, currently line 12; dark block, currently line 145).
2. `--color-sidebar:` → `--color-chrome:` (light block line 16; dark block line 149).
3. `--color-border-soft:` → `--color-border-subtle:` (light block line 17; dark block line 150).

Then add the two new hover tokens. In the **light** `:root` block, immediately after the `--color-border-strong:` declaration:

```css
  --color-hover: rgba(15, 23, 42, 0.04);
  --color-hover-strong: rgba(15, 23, 42, 0.07);
```

In the **dark** `:root[data-theme="dark"]` block, immediately after the dark `--color-border-strong:` declaration:

```css
  --color-hover: rgba(255, 255, 255, 0.05);
  --color-hover-strong: rgba(255, 255, 255, 0.08);
```

- [ ] **Step 4: Rename consumers (mechanical)**

Run from the repo root (the strings are distinct; these substitutions are safe). **Include the TS consumers** — `chartTokens.ts` / `LiveCurvePanel.tsx` resolve `--color-border-soft` at runtime and their tests assert the literal string; a CSS-only rename would leave dangling refs that silently break chart theming:

```bash
for f in apps/gui/src/styles/surfaces.css apps/gui/src/styles/components.css apps/gui/src/styles/pages.css \
         apps/gui/src/lib/chartTokens.ts apps/gui/src/lib/chartTokens.test.ts \
         apps/gui/src/components/overview/LiveCurvePanel.tsx \
         apps/gui/src/components/overview/LiveCurvePanel.test.tsx; do
  perl -0pi -e 's/--color-border-soft/--color-border-subtle/g; s/--color-canvas-subtle/--color-surface-subtle/g; s/--color-sidebar/--color-chrome/g' "$f"
done
```

(The `canvas-subtle` / `sidebar` patterns are no-ops in the TS files — only `border-soft` appears there — but running all three uniformly is harmless and keeps one loop. The JS property name `chartTokens.borderSoft` is NOT a token string and stays unchanged; only the `cssVar("--color-border-soft")` value mutates.)

Verify no stale names remain anywhere:

```bash
rg -n -e '--color-border-soft|--color-canvas-subtle|--color-sidebar' apps/gui/src --glob '!**/tokens.test.ts'
```

Expected: no output.

- [ ] **Step 5: Run test to verify it passes**

Run: `pnpm --filter @busytok/gui test -- src/styles/tokens.test.ts`
Expected: PASS.

- [ ] **Step 6: Verify build + typecheck + full suite**

Run: `pnpm --filter @busytok/gui typecheck && pnpm --filter @busytok/gui test`
Expected: typecheck clean; all tests PASS (no broken `var()` refs because CSS custom properties are forgiving, but the build confirms the CSS still compiles).

- [ ] **Step 7: Commit**

```bash
git add apps/gui/src/styles/tokens.css apps/gui/src/styles/tokens.test.ts apps/gui/src/styles/surfaces.css apps/gui/src/styles/components.css apps/gui/src/styles/pages.css apps/gui/src/lib/chartTokens.ts apps/gui/src/lib/chartTokens.test.ts apps/gui/src/components/overview/LiveCurvePanel.tsx apps/gui/src/components/overview/LiveCurvePanel.test.tsx
git commit -m "refactor(gui): rename border-soft/canvas-subtle/sidebar tokens; add hover tokens

Pure rename (values unchanged). Renames: --color-border-soft →
--color-border-subtle, --color-canvas-subtle → --color-surface-subtle,
--color-sidebar → --color-chrome. Adds --color-hover / --color-hover-strong
(defined, consumed in Phase 2). Phase 1 of the Geist refactor."
```

---

### Task 2: Collapse surface tiers + de-glass the light theme

**Files:**
- Modify: `apps/gui/src/styles/tokens.css` (light `:root` block)
- Modify: `apps/gui/src/styles/surfaces.css`, `apps/gui/src/styles/components.css`, `apps/gui/src/styles/pages.css`
- Test: `apps/gui/src/styles/tokens.test.ts`

**Interfaces:**
- Consumes: Task 1 names (`--color-surface`, `--color-surface-subtle`, `--color-chrome`, `--color-border-subtle`).
- Produces: light content surfaces become opaque; `surface-strong`/`surface-elevated` removed; chrome gets light vibrancy (`blur 8px`); resting card shadow → Geist scale.

- [ ] **Step 1: Write the failing contract test**

Add a new `it(...)` block to the `describe("tokens.css contract", ...)` in `apps/gui/src/styles/tokens.test.ts`:

```ts
  it("light theme: opaque surfaces, chrome vibrancy, Geist shadow (Phase 1)", () => {
    // content surface is opaque white, not translucent
    expect(tokensCss).toContain("--color-surface: #FFFFFF;");
    expect(tokensCss).toContain("--color-surface-subtle: #F7F8FA;");
    expect(tokensCss).toContain("--color-canvas: #F4F5F7;");
    expect(tokensCss).toContain("--color-chrome: rgba(255, 255, 255, 0.94);");
    // collapsed tiers are gone
    expect(tokensCss).not.toContain("--color-surface-strong:");
    expect(tokensCss).not.toContain("--color-surface-elevated:");
    // chrome blur only; subtle (content/scrim) blur is 0 per spec §4.1 —
    // modal-backdrop blur becomes a Phase 2 per-component concern
    expect(tokensCss).toContain("--material-glass-blur: 8px;");
    expect(tokensCss).toContain("--material-glass-blur-strong: 8px;");
    expect(tokensCss).toContain("--material-glass-blur-subtle: 0px;");
    // dead translucent-alpha tokens removed (no consumers)
    expect(tokensCss).not.toContain("--material-surface-alpha:");
    expect(tokensCss).not.toContain("--material-surface-strong-alpha:");
    // Geist raised-card shadow
    expect(tokensCss).toContain(
      "--material-shadow-card: 0 2px 2px rgba(15, 23, 42, 0.04);",
    );
    // text de-blued
    expect(tokensCss).toContain("--color-text: #1A1D23;");
  });
```

Also **remove** the now-stale assertion in the first `it(...)` block: delete the line `expect(css).toContain("--color-surface-elevated:");` (it asserts a token we are removing).

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm --filter @busytok/gui test -- src/styles/tokens.test.ts`
Expected: FAIL (`--color-surface: #FFFFFF;` not yet present).

- [ ] **Step 3: Rewrite the light neutral + material tokens**

In `apps/gui/src/styles/tokens.css`, **light `:root` block**, replace these declarations (currently lines 11–20 and 96–97) with:

```css
  --color-canvas: #F4F5F7;
  --color-surface-subtle: #F7F8FA;
  --color-surface: #FFFFFF;
  --color-chrome: rgba(255, 255, 255, 0.94);
  --color-border-subtle: rgba(15, 23, 42, 0.07);
  --color-border: rgba(15, 23, 42, 0.12);
  --color-border-strong: rgba(15, 23, 42, 0.20);
  --color-text: #1A1D23;
```

(Delete the old `--color-surface-strong` and `--color-surface-elevated` light declarations entirely.)

Replace the light material declarations (currently lines 91–97) with:

```css
  --material-glass-blur: 8px;
  --material-glass-blur-strong: 8px;
  --material-glass-blur-subtle: 0px;
  --material-glass-saturate: 1.08;
  --material-overlay-scrim: rgba(17, 24, 39, 0.32);
  --material-shadow-card: 0 2px 2px rgba(15, 23, 42, 0.04);
```

Then **delete the two dead alpha tokens** (currently lines 85–86 — they encoded the old translucent-surface alphas and have zero consumers once the tiers collapse):

```css
  --material-surface-alpha: 0.85;          /* delete */
  --material-surface-strong-alpha: 0.96;   /* delete */
```

(Leave light `--material-shadow-elevated` and `--material-tint-*` untouched this task — elevated shadow becomes the Geist popover in Task 3 alongside dark.)

- [ ] **Step 4: Repoint collapsed-tier consumers**

Run from repo root. **Include the TS consumers** — `nivoTheme.ts` (tooltip background) and `LiveCurvePanel.tsx` (loading overlay) resolve `--color-surface-strong` at runtime:

```bash
for f in apps/gui/src/styles/surfaces.css apps/gui/src/styles/components.css apps/gui/src/styles/pages.css \
         apps/gui/src/lib/nivoTheme.ts apps/gui/src/components/overview/LiveCurvePanel.tsx; do
  perl -0pi -e 's/--color-surface-strong/--color-surface/g; s/--color-surface-elevated/--color-surface-subtle/g' "$f"
done
rg -n -e '--color-surface-strong|--color-surface-elevated' apps/gui/src --glob '!**/tokens.test.ts'
```

Expected: the final `rg` produces no matches (the `--color-` prefix keeps `--material-surface-strong-alpha` out of the substitution, and that token is deleted in Step 3 anyway). If any match remains it is a comment; update it to the new vocabulary.

- [ ] **Step 5: Drop `backdrop-filter` from `.page-surface`**

In `apps/gui/src/styles/surfaces.css`, in the `.page-surface` rule, delete the line:

```css
  backdrop-filter: blur(var(--material-glass-blur)) saturate(var(--material-glass-saturate));
```

(`.page-surface` is a content surface — content surfaces are opaque and blur-free per the material contract. Chrome/modal-backdrop blur in `components.css` stays.)

- [ ] **Step 6: Run test + guard + build**

Run: `pnpm --filter @busytok/gui test -- src/styles/tokens.test.ts && pnpm --filter @busytok/gui build`
Expected: test PASS; build succeeds (Vite compiles the CSS).

- [ ] **Step 7: Commit**

```bash
git add apps/gui/src/styles/tokens.css apps/gui/src/styles/tokens.test.ts apps/gui/src/styles/surfaces.css apps/gui/src/styles/components.css apps/gui/src/styles/pages.css apps/gui/src/lib/nivoTheme.ts apps/gui/src/components/overview/LiveCurvePanel.tsx
git commit -m "refactor(gui): collapse surface tiers, de-glass light theme

Light content surfaces become opaque (#FFFFFF / #F7F8FA); surface-strong
and surface-elevated removed and repointed to surface / surface-subtle.
Chrome gets light vibrancy (blur 8px); page-surface backdrop-filter
dropped. Resting card shadow → Geist 0 2px 2px. Canvas #F4F5F7, text
#1A1D23. Phase 1 of the Geist refactor."
```

---

### Task 3: Temper the dark theme (opaque surfaces + Geist shadow)

**Files:**
- Modify: `apps/gui/src/styles/tokens.css` (dark block + shared elevated shadow)
- Test: `apps/gui/src/styles/tokens.test.ts`

**Interfaces:**
- Consumes: Task 2 light decisions; existing dark-blur-zero test.
- Produces: dark content surface opaque `#171C24`; subtle `#202732`; chrome near-opaque; Geist dark shadow scales. Dark blur stays `0` (most conservative; keeps the existing dark-blur test green and honors "dark chrome blur is supporting-only").

- [ ] **Step 1: Write the failing contract test**

Add to `apps/gui/src/styles/tokens.test.ts`:

```ts
  it("dark theme: opaque surfaces, Geist shadow, blur stays zero (Phase 1)", () => {
    const start = tokensCss.indexOf(':root[data-theme="dark"]');
    expect(start).toBeGreaterThan(-1);
    const dark = tokensCss.slice(start);
    expect(dark).toContain("--color-surface: #171C24;");
    expect(dark).toContain("--color-surface-subtle: #202732;");
    expect(dark).toContain("--color-chrome: rgba(22, 27, 34, 0.96);");
    expect(dark).toContain("--material-shadow-card: 0 1px 2px rgba(0, 0, 0, 0.16);");
    // dark blur remains zero (supporting-only → 0 for maximum calm)
    expect(dark).toContain("--material-glass-blur: 0px;");
    expect(dark).toContain("--material-glass-blur-strong: 0px;");
    expect(dark).toContain("--material-glass-blur-subtle: 0px;");
    // collapsed tiers gone from dark block too
    expect(dark).not.toContain("--color-surface-strong:");
    expect(dark).not.toContain("--color-surface-elevated:");
  });

  it("elevated shadow is the Geist popover stack (floating layers only)", () => {
    const popover =
      "0 1px 1px rgba(0, 0, 0, 0.02), 0 4px 8px -4px rgba(0, 0, 0, 0.04), 0 16px 24px -8px rgba(0, 0, 0, 0.06)";
    expect(tokensCss).toContain(`--material-shadow-elevated: ${popover};`);
  });
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm --filter @busytok/gui test -- src/styles/tokens.test.ts`
Expected: FAIL (`--color-surface: #171C24;` not present).

- [ ] **Step 3: Rewrite dark neutral tokens**

In `apps/gui/src/styles/tokens.css`, **dark block**, replace the neutral declarations (currently lines 144–152) with:

```css
  --color-canvas: #0D1117;
  --color-surface-subtle: #202732;
  --color-surface: #171C24;
  --color-chrome: rgba(22, 27, 34, 0.96);
  --color-border-subtle: rgba(255, 255, 255, 0.06);
  --color-border: rgba(255, 255, 255, 0.10);
  --color-border-strong: rgba(255, 255, 255, 0.16);
```

(Delete dark `--color-surface-strong` and `--color-surface-elevated`.)

- [ ] **Step 4: Rewrite dark material shadow; set Geist elevated popover (both themes)**

In the dark block, replace the dark shadow declarations (currently lines 208–209) with:

```css
  --material-shadow-card: 0 1px 2px rgba(0, 0, 0, 0.16);
  --material-shadow-elevated: 0 1px 1px rgba(0, 0, 0, 0.02), 0 4px 8px -4px rgba(0, 0, 0, 0.04), 0 16px 24px -8px rgba(0, 0, 0, 0.06);
```

In the **light** block, replace the light `--material-shadow-elevated` declaration (currently line 97) with the same popover stack value:

```css
  --material-shadow-elevated: 0 1px 1px rgba(0, 0, 0, 0.02), 0 4px 8px -4px rgba(0, 0, 0, 0.04), 0 16px 24px -8px rgba(0, 0, 0, 0.06);
```

- [ ] **Step 5: Run test to verify it passes**

Run: `pnpm --filter @busytok/gui test -- src/styles/tokens.test.ts`
Expected: PASS (including the pre-existing `zeroes glass blur ... in dark theme` test, which still holds).

- [ ] **Step 6: Commit**

```bash
git add apps/gui/src/styles/tokens.css apps/gui/src/styles/tokens.test.ts
git commit -m "refactor(gui): temper dark theme — opaque surfaces, Geist shadow

Dark content surface #171C24 (opaque), subtle #202732, chrome
rgba(22,27,34,.96). Elevated shadow → Geist popover stack (both themes),
reserved for floating layers only. Dark blur stays 0. Phase 1."
```

---

### Task 4: Radius role map — collapse outliers to 6/12/16

**Files:**
- Modify: `apps/gui/src/styles/tokens.css` (radius block)
- Modify: `apps/gui/src/styles/surfaces.css`, `apps/gui/src/styles/components.css`, `apps/gui/src/styles/pages.css`
- Test: `apps/gui/src/styles/tokens.test.ts`, `scripts/check-busytok-gui-surfaces.sh`

**Interfaces:**
- Produces: `--radius-sm: 6px` (controls/chips/inputs/segmented/keycaps/sidebar-items), `--radius-md: 12px` (cards/panels/popovers/menus), `--radius-lg: 16px` (dialogs/drawers/palette shell/page-surface), `--radius-pill: 999px`. `--radius-xs`/`--radius-xl` removed. Heatmap cells keep literal `3px` (documented exception).

- [ ] **Step 1: Write the failing contract test**

Add to `apps/gui/src/styles/tokens.test.ts`:

```ts
  it("radius role map: 6/12/16/pill, xs and xl removed (Phase 1)", () => {
    expect(tokensCss).toContain("--radius-sm: 6px;");
    expect(tokensCss).toContain("--radius-md: 12px;");
    expect(tokensCss).toContain("--radius-lg: 16px;");
    expect(tokensCss).toContain("--radius-pill: 999px;");
    expect(tokensCss).not.toContain("--radius-xs:");
    expect(tokensCss).not.toContain("--radius-xl:");
  });
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm --filter @busytok/gui test -- src/styles/tokens.test.ts`
Expected: FAIL (`--radius-sm: 6px;` not present).

- [ ] **Step 3: Rewrite the radius block**

In `apps/gui/src/styles/tokens.css`, replace the entire radius block (currently lines 109–115) with:

```css
  /* ── Radii — role map (6 control / 12 card / 16 dialog) ─────── */
  --radius-sm: 6px;
  --radius-md: 12px;
  --radius-lg: 16px;
  --radius-pill: 999px;
```

- [ ] **Step 4: Migrate the single `--radius-xl` consumer + remove outlier literals**

`.page-surface` in `apps/gui/src/styles/surfaces.css` uses `--radius-xl` → change to `--radius-lg`:

```bash
perl -0pi -e 's/var\(--radius-xl\)/var(--radius-lg)/g' apps/gui/src/styles/surfaces.css
```

Replace outlier literals in consumer CSS (heatmap cell `3px` is intentionally excluded — it is not in the set):

```bash
for f in apps/gui/src/styles/components.css apps/gui/src/styles/pages.css; do
  perl -0pi -e 's/border-radius:\s*22px/border-radius: var(--radius-lg)/g; s/border-radius:\s*26px/border-radius: var(--radius-lg)/g; s/border-radius:\s*32px/border-radius: var(--radius-lg)/g; s/border-radius:\s*24px/border-radius: var(--radius-md)/g' "$f"
done
```

(`prompt-overlay__surface` 32px (and its responsive `26px` override) and `prompt-dialog`/`confirm-dialog` 22px → `--radius-lg` 16; trend/live card 24px → `--radius-md` 12. These align with spec §7.3/§6.4. The palette row radius and heatmap are Phase 2 — not touched here.)

Verify no outlier literals remain:

```bash
rg -n -e 'border-radius:\s*(18|20|22|24|26|32)px' apps/gui/src/styles
```

Expected: no output.

- [ ] **Step 5: Add a guard rule (extend existing infra)**

In `scripts/check-busytok-gui-surfaces.sh`, append after the existing GUI-surface `rg` block (after the `fi` on line 11):

```bash
# ── Geist refactor Phase 1: radius outliers forbidden ───────────────
if rg -n -e 'border-radius:[[:space:]]*(18|20|22|24|26|32)px' apps/gui/src --glob '*.css'; then
  echo "Forbidden radius outlier (18/20/22/24/26/32) in CSS — use --radius-sm/md/lg"
  exit 1
fi
```

- [ ] **Step 6: Run test + guard + build**

Run: `pnpm --filter @busytok/gui test -- src/styles/tokens.test.ts && pnpm check:gui-surfaces && pnpm --filter @busytok/gui build`
Expected: test PASS; guard exits 0; build succeeds.

- [ ] **Step 7: Commit**

```bash
git add apps/gui/src/styles/tokens.css apps/gui/src/styles/tokens.test.ts apps/gui/src/styles/surfaces.css apps/gui/src/styles/components.css apps/gui/src/styles/pages.css scripts/check-busytok-gui-surfaces.sh
git commit -m "refactor(gui): radius role map 6/12/16, drop xs/xl + outliers

--radius-sm 6 (control/chip/input) / -md 12 (card/panel) / -lg 16
(dialog/drawer/palette shell). Removes --radius-xs/--radius-xl;
repoints page-surface (xl→lg) and replaces 22/24/32px literals.
Guard forbids 18/20/22/24/32 outliers. Phase 1."
```

---

### Task 5: `shadow-elevated` is floating-only — move resting panels off it

**Files:**
- Modify: `apps/gui/src/styles/pages.css`
- Test: `scripts/check-busytok-gui-surfaces.sh`

**Interfaces:**
- Consumes: Task 3 elevated popover shadow.
- Produces: resting panels (Usage Trend, Real-time Throughput, Heatmap) use the resting card shadow (or none), reserving `--material-shadow-elevated` for popover/dialog/drawer/menu/tooltip only.

- [ ] **Step 1: Write the failing guard rule**

In `scripts/check-busytok-gui-surfaces.sh`, append:

```bash
# ── Geist refactor Phase 1: shadow-elevated is floating-only ────────
# Resting overview panels must not carry the elevated (popover/dialog)
# shadow. The pattern `selector [^{]*\{ [^}]* shadow-elevated` bounds the
# match to a single CSS rule block, correlating the selector with its own
# box-shadow without false-matching the tooltip rules further down.
if rg -nU -e '(\.overview-console__trend|\.live-curve-panel|\.overview-heatmap)[^{]*\{[^}]*--material-shadow-elevated' apps/gui/src/styles/pages.css; then
  echo "Resting overview panel uses --material-shadow-elevated (floating-only)"
  exit 1
fi
```

- [ ] **Step 2: Run guard to verify it fails**

Run: `pnpm check:gui-surfaces`
Expected: FAIL — `.overview-console__trend, .live-curve-panel` (pages.css) and `.overview-heatmap` (pages.css) currently declare `box-shadow: var(--material-shadow-elevated);`.

- [ ] **Step 3: Move resting panels to the resting shadow**

In `apps/gui/src/styles/pages.css`:

1. In the `.overview-console__trend, .live-curve-panel` rule (currently lines 34–45), change:
   ```css
     box-shadow: var(--material-shadow-elevated);
   ```
   to:
   ```css
     box-shadow: var(--material-shadow-card);
   ```

2. In the `.overview-heatmap` rule (currently lines 926–932), make the same change (`--material-shadow-elevated` → `--material-shadow-card`).

(Per spec §5.2, resting panels are border-first, shadow optional; the small card shadow is the heaviest a resting surface gets. Tier-A "hardness" comes from `--color-border`, applied in Phase 2.)

- [ ] **Step 4: Run guard to verify it passes; run full suite**

Run: `pnpm check:gui-surfaces && pnpm --filter @busytok/gui test`
Expected: guard exits 0; tests PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/gui/src/styles/pages.css scripts/check-busytok-gui-surfaces.sh
git commit -m "refactor(gui): reserve shadow-elevated for floating layers

Trend / live-curve / heatmap (resting panels) now use the small card
shadow; --material-shadow-elevated is reserved for popover/dialog/drawer/
menu/tooltip. Guard enforces. Phase 1."
```

---

### Task 6: Heatmap-empty tempering (neutral substrate)

**Files:**
- Modify: `apps/gui/src/styles/tokens.css`
- Test: `apps/gui/src/styles/tokens.test.ts`

**Interfaces:**
- Produces: `--color-heatmap-empty` aligned to the neutral ladder (light `#EDF0F3`, dark `#202732`) so "no activity" reads as quiet substrate, distinct from the L1 indigo step.

- [ ] **Step 1: Update the failing test (it currently asserts the old values)**

In `apps/gui/src/styles/tokens.test.ts`, in the heatmap test block (currently the `it("defines dedicated heatmap tokens...")` block), change the two empty-substrate assertions:

```ts
    expect(tokensCss).toContain("--color-heatmap-empty: #EDF0F3;");
```

and in the dark slice:

```ts
    expect(darkBlock).toContain("--color-heatmap-empty: #202732;");
```

(Replace the old `#ebedf2;` light and `#232a35;` dark assertions. The L1–L4 ramp assertions stay unchanged.)

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm --filter @busytok/gui test -- src/styles/tokens.test.ts`
Expected: FAIL (`#EDF0F3` not present).

- [ ] **Step 3: Update the token values**

In `apps/gui/src/styles/tokens.css`:

- Light `:root`: `--color-heatmap-empty: #ebedf2;` → `--color-heatmap-empty: #EDF0F3;`
- Dark block: `--color-heatmap-empty: #232a35;` → `--color-heatmap-empty: #202732;`

(Aligns with `--color-surface-subtle` so the empty cell is a neutral substrate, not a data color. The L1–L4 indigo ramp is unchanged.)

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm --filter @busytok/gui test -- src/styles/tokens.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/gui/src/styles/tokens.css apps/gui/src/styles/tokens.test.ts
git commit -m "refactor(gui): heatmap empty cell → neutral substrate

--color-heatmap-empty aligns to the neutral ladder (light #EDF0F3, dark
#202732) so 'no activity' reads as quiet substrate, distinct from the
L1 indigo step. Phase 1."
```

---

### Task 7: Observability — design-system correlation marker

**Files:**
- Create: `apps/gui/src/logging/designSystem.ts`
- Create: `apps/gui/src/logging/designSystem.test.ts`
- Modify: `apps/gui/src/main.tsx`

**Interfaces:**
- Consumes: `safeReportEvent` from `apps/gui/src/logging/reporter.ts` (canonical fire-and-forget INFO wrapper).
- Produces: `reportDesignSystemApplied()` — emits a `gui.design_system.applied` event with the token-layer version, so field-reported visual behavior can be correlated to this refactor. DRY: one constant, one exported function. Wired **main-window-only** in `main.tsx` (the prompt-palette window shares the bootstrap; gating matches the existing updater pattern) so it fires once per app launch, not once per palette-window open.

- [ ] **Step 1: Write the failing test**

Create `apps/gui/src/logging/designSystem.test.ts`:

```ts
import { beforeEach, describe, expect, it, vi } from "vitest";

// vi.hoisted is the repo's mock idiom (see reporter.test.ts /
// PromptPalettePage.test.tsx): it binds the mock before vi.mock's hoisted
// factory runs, so there is no TDZ on the `reportMock` reference.
const { reportMock } = vi.hoisted(() => ({ reportMock: vi.fn() }));

vi.mock("./reporter", () => ({
  safeReportEvent: reportMock,
}));

import { DESIGN_SYSTEM_VERSION, reportDesignSystemApplied } from "./designSystem";

describe("reportDesignSystemApplied", () => {
  beforeEach(() => {
    reportMock.mockClear();
  });

  it("emits a single gui.design_system.applied INFO event with the version", () => {
    reportDesignSystemApplied();
    expect(reportMock).toHaveBeenCalledTimes(1);
    expect(reportMock).toHaveBeenCalledWith(
      "gui.design_system.applied",
      "Design system token layer applied",
      { version: DESIGN_SYSTEM_VERSION },
    );
  });

  it("never throws (observability must not break bootstrap)", () => {
    reportMock.mockImplementation(() => {
      throw new Error("reporter down");
    });
    expect(() => reportDesignSystemApplied()).not.toThrow();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm --filter @busytok/gui test -- src/logging/designSystem.test.ts`
Expected: FAIL — `./designSystem` does not exist.

- [ ] **Step 3: Implement the module**

Create `apps/gui/src/logging/designSystem.ts`:

```ts
// apps/gui/src/logging/designSystem.ts

import { safeReportEvent } from "./reporter";

/**
 * Token-layer version tag for the Geist-inspired refactor. Emitted once per
 * bootstrap so that field-reported visual behavior can be correlated to the
 * active design-system contract. Bump this when a Phase lands.
 */
export const DESIGN_SYSTEM_VERSION = "geist-refactor-phase-1";

/**
 * Fire-and-forget marker that the design-system token layer is active.
 * Safe to call from the bootstrap path — never throws into app startup.
 */
export function reportDesignSystemApplied(): void {
  try {
    safeReportEvent(
      "gui.design_system.applied",
      "Design system token layer applied",
      { version: DESIGN_SYSTEM_VERSION },
    );
  } catch {
    // Observability must not break bootstrap.
  }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm --filter @busytok/gui test -- src/logging/designSystem.test.ts`
Expected: PASS.

- [ ] **Step 5: Wire it into bootstrap (main window only)**

In `apps/gui/src/main.tsx`, add the import alongside the existing logging/theme imports:

```ts
import { reportDesignSystemApplied } from "./logging/designSystem";
```

`main.tsx` is the **shared** bootstrap for both the main app window and the prompt-palette window (distinguished by `promptPaletteWindow`, currently line 27). `initThemeRuntime()` runs for both. To emit the marker **once per app launch (main window only)** — not once per palette-window open — gate it exactly like the updater (`if (!promptPaletteWindow)`, currently line 47). Add this block immediately after the `initThemeRuntime();` call:

```ts
// Design-system token-layer marker — main app only. The prompt-palette
// window is a child of the same app/version, so it does not emit its own.
if (!promptPaletteWindow) {
  reportDesignSystemApplied();
}
```

- [ ] **Step 6: Run typecheck + full suite**

Run: `pnpm --filter @busytok/gui typecheck && pnpm --filter @busytok/gui test`
Expected: typecheck clean; all tests PASS.

- [ ] **Step 7: Commit**

```bash
git add apps/gui/src/logging/designSystem.ts apps/gui/src/logging/designSystem.test.ts apps/gui/src/main.tsx
git commit -m "feat(gui): design-system correlation telemetry marker

reportDesignSystemApplied() emits gui.design_system.applied once per
main-window bootstrap (gated like the updater; the prompt-palette window
shares bootstrap but does not emit) with DESIGN_SYSTEM_VERSION, so
field-reported visual behavior can be correlated to the active token
layer. Reuses safeReportEvent; never throws into bootstrap. Phase 1."
```

---

### Task 8: Guard consolidation + full Phase 1 verification

**Files:**
- Modify: `scripts/check-busytok-gui-surfaces.sh`

**Interfaces:**
- Produces: the guard asserts the full Phase 1 contract — no stale token names, `backdrop-filter` only in chrome/modal selectors, no radius outliers, no raw hex outside `tokens.css`. Combined with `tokens.test.ts` this is the regression net for Phase 1.

- [ ] **Step 1: Write the guard rules**

In `scripts/check-busytok-gui-surfaces.sh`, append (after the radius block added in Task 4):

```bash
# ── Geist refactor Phase 1: stale token names forbidden (CSS + TS) ───
# Scans all of apps/gui/src, not just CSS — chartTokens.ts / nivoTheme.ts /
# LiveCurvePanel.tsx consume tokens at runtime and must migrate too.
if rg -n -e '--color-surface-strong|--color-surface-elevated|--color-canvas-subtle|--color-border-soft|--color-sidebar|--radius-xs|--radius-xl' \
  apps/gui/src --glob '!**/tokens.css' --glob '!**/tokens.test.ts'; then
  echo "Found stale/removed token name"
  exit 1
fi

# ── Geist refactor Phase 1: backdrop-filter is chrome/modal-only ─────
# Positive allowlist (spec §3): backdrop-filter may appear ONLY inside a
# rule whose selector is .desktop-sidebar / .desktop-titlebar /
# .prompt-dialog__overlay / .confirm-dialog__overlay / .prompt-overlay__backdrop.
# The awk tracks the current selector (set on `{`, reset on `}`) so each
# backdrop-filter is correlated with its own rule — a new content component
# added to components.css that sneaks in a blur is caught, not just
# surfaces.css/pages.css.
# Assumption: selector and `{` are on one line (true for current CSS style);
# if CSS later moves to multi-line selectors, extend this script to track
# the full selector across lines.
if ! awk '
  /\}/ { sel = ""; next }
  /\{/ { sel = $0; sub(/\{.*/, "", sel); next }
  /backdrop-filter/ {
    if (sel ~ /\.desktop-sidebar/ || sel ~ /\.desktop-titlebar/ || sel ~ /\.prompt-dialog__overlay/ || sel ~ /\.confirm-dialog__overlay/ || sel ~ /\.prompt-overlay__backdrop/) next
    print FILENAME ": backdrop-filter outside chrome/modal allowlist: " sel; bad = 1
  }
  END { exit bad ? 1 : 0 }
' apps/gui/src/styles/surfaces.css apps/gui/src/styles/components.css apps/gui/src/styles/pages.css; then
  echo "backdrop-filter outside chrome/modal allowlist (spec §3)"
  exit 1
fi

# ── Geist refactor Phase 1: no raw hex in CSS consumer files ─────────
# Scope: CSS consumer layer only (spec §8.3). TS chart-runtime fallback
# colors — e.g. LiveCurvePanel.tsx resolveCssColor("--color-data-live-
# primary", "#4f63f6") — are the spec §8.3 "third-party chart-lib inline
# fallback" whitelist case and are intentionally OUT of this guard's scope.
# If a CSS consumer needs a color, consume a token.
if rg -n --glob '*.css' --glob '!tokens.css' -e '#[0-9a-fA-F]{3,8}' apps/gui/src/styles; then
  echo "Raw hex in CSS consumer file — consume a token"
  exit 1
fi
```

- [ ] **Step 2: Run guard to verify current state**

Run: `pnpm check:gui-surfaces`
Expected: exit 0. (Audited: there are currently zero raw-hex literals in `surfaces.css` / `components.css` / `pages.css`, so all three new rules pass cleanly on application.) If a future change introduces a raw hex in a consumer file, migrate it to a token rather than whitelisting the path.

- [ ] **Step 3: Full Phase 1 verification gate**

Run (all must pass):

```bash
pnpm --filter @busytok/gui typecheck
pnpm --filter @busytok/gui test
pnpm coverage:gui
pnpm check:gui-surfaces
pnpm --filter @busytok/gui build
```

Expected: typecheck clean; all tests PASS; coverage ≥90% lines; guard exit 0; production build succeeds.

- [ ] **Step 4: Manual smoke (rendered baseline)**

Run: `pnpm dev:gui` (or the project's run skill). Confirm in both light and dark:
- App shell renders; sidebar/titlebar have light vibrancy, content panels are opaque (no "floating glass" over scrolling content).
- Overview trend/live/heatmap panels sit on opaque surfaces with hairline borders (no heavy elevated shadow).
- No layout breakage; no unstyled (`var()`-fallback) boxes.

- [ ] **Step 5: Commit**

```bash
git add scripts/check-busytok-gui-surfaces.sh
git commit -m "test(gui): guard stale tokens, content blur, radius outliers, raw hex

Extends check-busytok-gui-surfaces.sh to enforce the Phase 1 contract:
no removed/renamed token names in consumers, backdrop-filter only in
chrome/modal selectors, no radius outliers, no raw hex outside tokens.css
(default-deny + whitelist). Phase 1 verification gate green."
```

---

## Self-Review (run after writing — recorded here for the implementer)

**1. Spec coverage (Phase 1 scope = spec §2 Phase 1 + §3 + §4 + §5 rules that are token-level):**
- §3 material contract (opaque surfaces, chrome-only vibrancy, content blur ban) → Tasks 2, 5, guard (Task 8). ✓
- §4.1 light token diff → Task 2 (+ Task 1 renames, Task 4 radius, Task 6 heatmap). ✓
- §4.2 dark token diff → Task 3. ✓
- §4.3 radius role map → Task 4. ✓
- §4.4 data palette降温 — token-level: heatmap-empty (Task 6); teal/violet降频 is a *consumer* change (Phase 2), no token removed. ✓ (noted as Phase 2)
- §5 rules 1, 3, 4, 5, 8, 10 (token-enforceable) → Tasks 2/3/4/5 + guard. Rules 2, 6, 7, 9, 11–15 are *consumer/component* behavior → Phase 2. ✓
- Observability ask → Task 7. ✓

**2. Placeholder scan:** none — every step has exact paths, code, commands, expected output.

**3. Type consistency:** `DESIGN_SYSTEM_VERSION` / `reportDesignSystemApplied` defined in Task 7 step 3, imported in step 5, asserted in step 1 by the same names. `--color-hover` / `--color-hover-strong` defined Task 1, asserted Task 1, consumed Phase 2. Remap rules stated once in File Structure and reused verbatim in Tasks 1–2.

**4. Review-driven fixes applied (subagent review):**
- **TS consumers migrated** (P0): Tasks 1 & 2 substitution loops now include `chartTokens.ts` / `chartTokens.test.ts` / `nivoTheme.ts` / `LiveCurvePanel.tsx` / `LiveCurvePanel.test.tsx` — runtime CSS-var refs that would otherwise silently break chart theming.
- **Guard covers TS** (P0): Task 8 stale-token guard scans all of `apps/gui/src`, not just CSS.
- **Task 5 guard rewritten** (P0): the old `rg | rg` pipeline could never correlate a selector with its `box-shadow`; replaced with a multiline rule-bounded pattern.
- **`26px` radius** (P1): added to Task 4 perl + guard (responsive palette shell).
- **`--material-glass-blur-subtle` light → `0px`** (P1): follows spec §4.1; modal-backdrop blur deferred to Phase 2 per-component.
- **Dead tokens removed** (P1): `--material-surface-alpha` / `-strong-alpha` deleted in Task 2 (zero consumers).
- **Task 7 mock** (P1): uses repo idiom `vi.hoisted` + `./reporter`.
- **Task 8 raw-hex guard** (P2): dropped redundant `| rg -v` pipe.
- **Task 7 main-window gating** (P1, second review): `reportDesignSystemApplied()` is now wrapped in `if (!promptPaletteWindow)` — `main.tsx` is the shared bootstrap for both window types, so an ungated call would emit once per palette-window open, not once per app launch.
- **Task 8 backdrop-filter allowlist** (P1, second review): the file-name-only `rg` rule (surfaces/pages) was too weak — a new content component in `components.css` could sneak in a blur. Replaced with a positive awk allowlist that correlates each `backdrop-filter` with its enclosing selector; only the 5 chrome/modal selectors pass.
- **Task 8 raw-hex scope** (P2, second review): guard comment now states CSS-consumer-only scope; TS chart-runtime fallbacks (`LiveCurvePanel.tsx`) are the spec §8.3 whitelist and out of scope.

**Phase 2/3 outlook (separate plans, written after this lands):**
- **Phase 2** (component behavior): Titlebar single calm chip + read-only popover bound to `shell.status`; sidebar active rail + `--color-hover`; metric-card neutralization (delete `--success` variant, flag/dot exceptions); Overview 3-tier via border-strength (Tier A `--color-border`); chart readout-ification (single indigo line, ≤8% fill, no vertical grid, explicit chart-token stroke); rankings neutral bars + one accent; Prompt Palette command-surface across all 4 carriers; migrate hover states to `--color-hover`. Each component = its own task with component tests (Testing Library) + `safeReportEvent` hooks at escalation/surface-mode points.
- **Phase 3** (docs): create `DESIGN-SYSTEM.md` (SSOT), delete `THEME.md`, rewrite `DESIGN.md` visual section to a non-normative pointer, sweep stale `/* per spec */` CSS comments, promote the §5 rules + Do/Don't to the Review Checklist.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-23-busytok-geist-refactor-phase1-tokens.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
