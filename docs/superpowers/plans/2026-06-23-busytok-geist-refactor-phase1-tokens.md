# Busytok Geist Refactor — Phase 1: Token & Material Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite the GUI's token + material contract (`tokens.css`) to a de-glassed, opaque, Geist-calibrated foundation and migrate every consumer, with zero component-structure changes — producing a calmer baseline that still renders correctly and is fully test/guard-covered.

**Architecture:** Token-first (spec Phase 1). All visual calming cascades from `tokens.css`; the three consumer stylesheets (`surfaces/components/pages.css`) are migrated mechanically to the new vocabulary. The contract is enforced three ways: (1) `tokens.test.ts` asserts literal token values, (2) `scripts/check-busytok-gui-surfaces.sh` (existing bash `rg` guard) is extended to forbid stale token names / stray `backdrop-filter` / radius outliers / raw hex, (3) the existing 90% coverage gate. A one-shot design-system telemetry marker is emitted at bootstrap via the existing `reporter.ts` so any field-reported visual regression can be correlated to this token layer.

**Tech Stack:** React 19 + Vite 7 + Vitest 3 (string-contract tests over `tokens.css`), plain CSS custom properties, Tauri 2, `reporter.ts` (`safeReportEvent`) for observability, bash `rg` for guards.

## Global Constraints

(From spec `docs/superpowers/specs/2026-06-22-busytok-geist-refactor-design.md`. Every task implicitly includes these.)

- **No component-structure changes this phase.** Token values change and cascade; component logic/JSX is untouched (except the one bootstrap observability hook in Task 7).
- **No dead code.** Removed/renamed tokens are deleted everywhere — no compatibility aliases. (`surface-strong`, `surface-elevated`, `canvas-subtle`, `border-soft`, `sidebar`, `radius-xs`, `radius-xl` must be gone after Phase 1.)
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
| `scripts/check-busytok-gui-surfaces.sh` | bash `rg` regression guard | Add stale-token / blur / radius / hex rules |
| `apps/gui/src/logging/designSystem.ts` | **New** — design-system version marker | Created (Task 7) |
| `apps/gui/src/logging/designSystem.test.ts` | **New** — marker test | Created (Task 7) |
| `apps/gui/src/main.tsx` | Bootstrap | Call marker once after `initThemeRuntime()` (Task 7) |

**Remap rules (locked, used across Tasks 1–2):**
- `--color-surface-strong` → `--color-surface` (floating/strong bodies → primary opaque surface)
- `--color-surface-elevated` → `--color-surface-subtle` (raised/hover fills → subtle tier)
- `--color-canvas-subtle` → `--color-surface-subtle` (rename)
- `--color-border-soft` → `--color-border-subtle` (rename)
- `--color-sidebar` → `--color-chrome` (rename)

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

Run from the repo root (the strings are distinct; these substitutions are safe):

```bash
for f in apps/gui/src/styles/surfaces.css apps/gui/src/styles/components.css apps/gui/src/styles/pages.css; do
  perl -0pi -e 's/--color-border-soft/--color-border-subtle/g; s/--color-canvas-subtle/--color-surface-subtle/g; s/--color-sidebar/--color-chrome/g' "$f"
done
```

Verify no stale names remain in consumers:

```bash
rg -n --color-border-soft|--color-canvas-subtle|--color-sidebar apps/gui/src/styles/surfaces.css apps/gui/src/styles/components.css apps/gui/src/styles/pages.css
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
git add apps/gui/src/styles/tokens.css apps/gui/src/styles/tokens.test.ts apps/gui/src/styles/surfaces.css apps/gui/src/styles/components.css apps/gui/src/styles/pages.css
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
    // chrome blur only; subtle scrim blur retained for modal backdrops
    expect(tokensCss).toContain("--material-glass-blur: 8px;");
    expect(tokensCss).toContain("--material-glass-blur-strong: 8px;");
    expect(tokensCss).toContain("--material-glass-blur-subtle: 6px;");
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
  --material-glass-blur-subtle: 6px;
  --material-glass-saturate: 1.08;
  --material-overlay-scrim: rgba(17, 24, 39, 0.32);
  --material-shadow-card: 0 2px 2px rgba(15, 23, 42, 0.04);
```

(Leave light `--material-shadow-elevated`, `--material-tint-*`, `--material-surface-alpha*` untouched this task — elevated shadow becomes Geist popover in Task 3 alongside dark.)

- [ ] **Step 4: Repoint collapsed-tier consumers**

Run from repo root:

```bash
for f in apps/gui/src/styles/surfaces.css apps/gui/src/styles/components.css apps/gui/src/styles/pages.css; do
  perl -0pi -e 's/--color-surface-strong/--color-surface/g; s/--color-surface-elevated/--color-surface-subtle/g' "$f"
done
rg -n --color-surface-strong|--color-surface-elevated apps/gui/src/styles
```

Expected: final `rg` prints only the (now-deleted) absence — i.e. no matches. If any match remains, it is a comment; update the comment text to the new vocabulary.

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
git add apps/gui/src/styles/tokens.css apps/gui/src/styles/tokens.test.ts apps/gui/src/styles/surfaces.css apps/gui/src/styles/components.css apps/gui/src/styles/pages.css
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

Replace outlier literals in consumer CSS (heatmap cell `3px` is intentionally excluded — it is not in the `(18|20|22|24|32)` set):

```bash
for f in apps/gui/src/styles/components.css apps/gui/src/styles/pages.css; do
  perl -0pi -e 's/border-radius:\s*22px/border-radius: var(--radius-lg)/g; s/border-radius:\s*32px/border-radius: var(--radius-lg)/g; s/border-radius:\s*24px/border-radius: var(--radius-md)/g' "$f"
done
```

(`prompt-overlay__surface` 32px and `prompt-dialog`/`confirm-dialog` 22px → `--radius-lg` 16; trend/live card 24px → `--radius-md` 12. These align with spec §7.3/§6.4. The palette row radius and heatmap are Phase 2 — not touched here.)

Verify no outlier literals remain:

```bash
rg -n 'border-radius:\s*(18|20|22|24|32)px' apps/gui/src/styles
```

Expected: no output.

- [ ] **Step 5: Add a guard rule (extend existing infra)**

In `scripts/check-busytok-gui-surfaces.sh`, append after the existing GUI-surface `rg` block (after the `fi` on line 11):

```bash
# ── Geist refactor Phase 1: radius outliers forbidden ───────────────
if rg -n 'border-radius:[[:space:]]*(18|20|22|24|32)px' apps/gui/src --glob '*.css'; then
  echo "Forbidden radius outlier (18/20/22/24/32) in CSS — use --radius-sm/md/lg"
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
# Resting panels must not carry the elevated (popover/dialog) shadow.
if rg -n -- '--material-shadow-elevated' apps/gui/src/styles/pages.css \
  | rg -i 'overview-console__trend|live-curve-panel|overview-heatmap'; then
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
- Produces: `reportDesignSystemApplied()` — emits a one-shot `gui.design_system.applied` event with the token-layer version, so field-reported visual behavior can be correlated to this refactor. DRY: one constant, one exported function, reused by bootstrap.

- [ ] **Step 1: Write the failing test**

Create `apps/gui/src/logging/designSystem.test.ts`:

```ts
import { beforeEach, describe, expect, it, vi } from "vitest";

const reportMock = vi.fn();
vi.mock("../logging/reporter", () => ({
  get safeReportEvent() {
    return reportMock;
  },
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

- [ ] **Step 5: Wire it into bootstrap (exactly once)**

In `apps/gui/src/main.tsx`, add the import alongside the existing logging/theme imports:

```ts
import { reportDesignSystemApplied } from "./logging/designSystem";
```

Immediately after the `initThemeRuntime();` call (which the existing bootstrap runs before first React render), add:

```ts
reportDesignSystemApplied();
```

(If `main.tsx` already wraps bootstrap in a try/catch or calls other `report*` helpers, place this call in the same bootstrap sequence so it fires exactly once per app launch.)

- [ ] **Step 6: Run typecheck + full suite**

Run: `pnpm --filter @busytok/gui typecheck && pnpm --filter @busytok/gui test`
Expected: typecheck clean; all tests PASS.

- [ ] **Step 7: Commit**

```bash
git add apps/gui/src/logging/designSystem.ts apps/gui/src/logging/designSystem.test.ts apps/gui/src/main.tsx
git commit -m "feat(gui): design-system correlation telemetry marker

reportDesignSystemApplied() emits gui.design_system.applied once at
bootstrap with DESIGN_SYSTEM_VERSION, so field-reported visual behavior
can be correlated to the active token layer. Reuses safeReportEvent;
never throws into bootstrap. Phase 1."
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
# ── Geist refactor Phase 1: stale token names forbidden in consumers ─
if rg -n -- \
  '--color-surface-strong|--color-surface-elevated|--color-canvas-subtle|--color-border-soft|--color-sidebar|--radius-xs|--radius-xl' \
  apps/gui/src/styles/surfaces.css apps/gui/src/styles/components.css apps/gui/src/styles/pages.css; then
  echo "Found stale/removed token name in consumer CSS"
  exit 1
fi

# ── Geist refactor Phase 1: backdrop-filter is chrome/modal-only ─────
# Allowed only in components.css (titlebar, sidebar, dialog/palette scrims).
if rg -n 'backdrop-filter' apps/gui/src/styles/surfaces.css apps/gui/src/styles/pages.css; then
  echo "backdrop-filter appears in surfaces.css/pages.css (content surfaces must be opaque)"
  exit 1
fi

# ── Geist refactor Phase 1: no raw hex outside tokens.css ────────────
# Default-deny. Whitelist (legitimate inline hex) lives in the exception
# below; expand it only with a documented reason.
hex_whitelist='apps/gui/src/styles/tokens.css'
if rg -n --glob '*.css' --glob '!tokens.css' '#[0-9a-fA-F]{3,8}\b' apps/gui/src/styles \
  | rg -v "$hex_whitelist"; then
  echo "Raw hex outside tokens.css — consume a token (or document a whitelist exception)"
  exit 1
fi
```

- [ ] **Step 2: Run guard to verify current state**

Run: `pnpm check:gui-surfaces`
Expected: exit 0. If the raw-hex rule fails, inspect each hit:
- If it is a legitimately-inline value (e.g. a third-party chart fallback or an opaque white inside a `color-mix`), migrate it to a token if possible; if migration is genuinely impossible, append the file to a documented whitelist comment above the rule and re-run.
- Do not silence the rule by deleting it.

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

**Phase 2/3 outlook (separate plans, written after this lands):**
- **Phase 2** (component behavior): Titlebar single calm chip + read-only popover bound to `shell.status`; sidebar active rail + `--color-hover`; metric-card neutralization (delete `--success` variant, flag/dot exceptions); Overview 3-tier via border-strength (Tier A `--color-border`); chart readout-ification (single indigo line, ≤8% fill, no vertical grid, explicit chart-token stroke); rankings neutral bars + one accent; Prompt Palette command-surface across all 4 carriers; migrate hover states to `--color-hover`. Each component = its own task with component tests (Testing Library) + `safeReportEvent` hooks at escalation/surface-mode points.
- **Phase 3** (docs): create `DESIGN-SYSTEM.md` (SSOT), delete `THEME.md`, rewrite `DESIGN.md` visual section to a non-normative pointer, sweep stale `/* per spec */` CSS comments, promote the §5 rules + Do/Don't to the Review Checklist.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-23-busytok-geist-refactor-phase1-tokens.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
