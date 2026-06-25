# Busytok Geist Refactor — Phase 3: Documentation & Cleanup Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a canonical visual contract (`DESIGN-SYSTEM.md`), delete the 100% abandoned `THEME.md` (551-line Sentri system), rewrite `DESIGN.md`'s visual section to a non-normative pointer, sweep stale CSS comments, and extend automated guards to cover the two remaining spec-enforceable rules — producing a single-source-of-truth design system with regression coverage.

**Architecture:** Phase 3 is pure docs + guard work. No component structure, no token values, no visual change. The centerpiece is `DESIGN-SYSTEM.md` — a ~350-line canonical contract that distills the spec (§1–§8) into a self-contained reference: governance → material → tokens → usage rules → component contracts → Review Checklist → Do/Don't. `DESIGN.md`'s "Visual design" section becomes a one-line pointer. `THEME.md` is deleted. The single stale `/* per spec */` CSS comment is reworded. Two guard rules (no `metric-card--success` class; dark accent text uses `accent-400` not 500/600) are added to the existing bash guard.

**Tech Stack:** Markdown, bash `rg`/`awk` (extending `scripts/check-busytok-gui-surfaces.sh`), CSS comment update. No TS/React changes.

## Global Constraints

(From spec `docs/superpowers/specs/2026-06-22-busytok-geist-refactor-design.md` §8. Every task implicitly includes these.)

- **`DESIGN-SYSTEM.md` = canonical visual contract.** `DESIGN.md` = narrative overview (non-normative). They co-evolve but `DESIGN-SYSTEM.md` is authoritative for visuals.
- **`THEME.md` is 100% abandoned** (Sentri system: violet-lime palette, Rubik/Monaco fonts, proprietary display sans — none of which Busytok uses). Delete it. No salvage.
- **No dead language left behind.** The Sentri description in `DESIGN.md` Visual design section is deleted. The `/* per spec */` comment in `components.css:311` is reworded to reference the design system by name.
- **Guard rules extend existing infra only.** `scripts/check-busytok-gui-surfaces.sh` gets 2 new rules. No new toolchain.
- **Coverage gate:** `pnpm coverage:gui` requires ≥90% lines. No TS changes this phase; coverage is unaffected.
- **Commit per task.** Run `pnpm --filter @busytok/gui typecheck && pnpm check:gui-surfaces && pnpm --filter @busytok/gui build` green before each commit. Token tests must stay green (`cd apps/gui && npx vitest run src/styles/tokens.test.ts`).

## File Structure

| File | Responsibility | This phase |
|---|---|---|
| `DESIGN-SYSTEM.md` | **New** — canonical visual contract (SSOT) | Created (Task 1) |
| `THEME.md` | Abandoned Sentri system | **Deleted** (Task 2) |
| `DESIGN.md` | Narrative architecture overview | "Visual design" section rewritten to pointer (Task 2) |
| `apps/gui/src/styles/components.css` | Sidebar/titlebar/chip/palette/table CSS | One comment reworded (Task 3) |
| `scripts/check-busytok-gui-surfaces.sh` | bash `rg` regression guard | Two new rules added (Task 4) |

**Key reuse points (do not re-create):**
- The spec (`docs/superpowers/specs/2026-06-22-busytok-geist-refactor-design.md`) is the source of truth for all DESIGN-SYSTEM.md content — copy/condense, don't invent.
- The Phase 1 guard (`scripts/check-busytok-gui-surfaces.sh`) is extended in-place — the existing stale-token, blur, radius, hex, and shadow-elevated rules are all kept.

---

### Task 1: Create `DESIGN-SYSTEM.md` — canonical visual contract

**Files:**
- Create: `DESIGN-SYSTEM.md`
- Verify: `scripts/check-busytok-gui-surfaces.sh` (Task 4 adds an existence check)

**Interfaces:**
- Consumes: spec §1–§8, Phase 1 token contract (`tokens.css`), Phase 2 component behavior.
- Produces: `DESIGN-SYSTEM.md` as SSOT for visual design. Referenced by `DESIGN.md` (Task 2) and the guard (Task 4). The Review Checklist within is the PR review gate for future visual changes.

- [ ] **Step 1: Write `DESIGN-SYSTEM.md`**

Create `DESIGN-SYSTEM.md` at repo root:

```markdown
# Busytok Design System

> **Canonical visual contract.** This document is authoritative for all GUI
> visual design. It co-evolves with `apps/gui/src/styles/tokens.css` (the
> executable contract) and is enforced by `tokens.test.ts` +
> `scripts/check-busytok-gui-surfaces.sh`.
>
> `DESIGN.md` is the narrative architecture overview — it is **non-normative**
> for visuals and defers to this document.

---

## 1. Governance Principles

1. **Default neutral, status scarce** — Healthy is quiet; only anomalies carry semantic color.
2. **Structure first, color second** — Hierarchy is built from the neutral surface ladder + borders + spacing; color is the last resort.
3. **Panels are tool containers, not visual protagonists** — Cards carry numbers; they don't compete with them.
4. **Real-time information must be clear, not loud** — Live curves can be slightly brighter but never glow alongside panels.

### Section-level summaries (review yardsticks)

- **Overall**: Default UI = opaque neutral content surfaces + minimal chrome; semantic color marks status only, never takes over structure.
- **Titlebar**: The titlebar conveys system-health **awareness**, not system-detail **display**.
- **Sidebar**: The sidebar should read like a **directory**, not a column of clickable cards.
- **Metric**: Metric cards present **readings**, not perform status.
- **Overview**: Overview hierarchy comes from **page rhythm and panel quietness**, not stacked material effects.
- **Charts**: Charts help **read numbers**, not create atmosphere.
- **Prompt Palette**: The palette's visual goal is **command precision**, not content browsing.
- **Dark**: Dark theme is a **more restrained reading environment**, not a flashier version.

---

## 2. Material Contract

> **Material stays on the shell; information lives only in opaque content surfaces.**

| Role | Light | Dark |
|---|---|---|
| Canvas (app background) | Opaque `#F4F5F7` | Opaque `#0D1117` |
| **Content surface** (cards/charts/details/dialogs) | **Opaque** `#FFFFFF` | **Opaque** `#171C24` |
| Subtle surface (nested/secondary separation) | Opaque `#F7F8FA` | Opaque `#202732` |
| **Chrome** (titlebar / sidebar) | Light vibrancy `rgba(255,255,255,.94)` + `blur 8px` | Near-opaque `rgba(22,27,34,.96)` + blur `0–4px` (supporting-only) |
| Blur in content area | **Forbidden** | **Forbidden** |

- The old 3-tier translucent surface ladder (`surface .85 / strong .96 / elevated .92`) is **collapsed into 2 opaque tiers** + 1 chrome tier.
- Dialogs/drawers/popovers always use opaque surfaces + shadows; never rely on blur for depth.
- Dark content surface `#171C24` (one step lighter than old `#161B22`) to separate from chrome.

### Semantic color exception hierarchy (fixed)

- `info / success` → dot / chip / small inline label
- `warning / degraded` → chip + 1px semantic border or left rail
- `danger / blocking` → stronger semantic container (semantic border)

---

## 3. Token Reference

The executable token contract lives in `apps/gui/src/styles/tokens.css`. This section documents the design intent behind key values.

### 3.1 Neutral Ladder

| Token | Light | Dark | Role |
|---|---|---|---|
| `--color-canvas` | `#F4F5F7` | `#0D1117` | App background |
| `--color-surface` | `#FFFFFF` | `#171C24` | Primary content surface |
| `--color-surface-subtle` | `#F7F8FA` | `#202732` | Secondary/embedded surface |
| `--color-chrome` | `rgba(255,255,255,.94)` | `rgba(22,27,34,.96)` | Titlebar + sidebar |
| `--color-border-subtle` | `rgba(15,23,42,.07)` | `rgba(255,255,255,.06)` | Resting panel border |
| `--color-border` | `rgba(15,23,42,.12)` | `rgba(255,255,255,.10)` | Tier-A / floating border |
| `--color-border-strong` | `rgba(15,23,42,.20)` | `rgba(255,255,255,.16)` | Focus / active border |
| `--color-hover` | `rgba(15,23,42,.04)` | `rgba(255,255,255,.05)` | Hover fill |
| `--color-hover-strong` | `rgba(15,23,42,.07)` | `rgba(255,255,255,.08)` | Selected/pressed fill |
| `--color-text` | `#1A1D23` | `#e6edf3` | Primary text |
| `--color-text-muted` | `#6b7280` | `#8b949e` | Secondary text |
| `--color-text-faint` | `#9ca3af` | `#6e7681` | Tertiary/label text |

### 3.2 Accent (Indigo)

| Token | Light | Dark |
|---|---|---|
| `--color-accent-50` | `#eef2ff` | `#1e1b4b` |
| `--color-accent-100` | `#e0e7ff` | `#312e81` |
| `--color-accent-400` | `#6366f1` | `#818cf8` |
| `--color-accent-500` | `#4f46e5` | `#6366f1` |
| `--color-accent-600` | `#4338ca` | `#4f46e5` |
| `--color-accent-700` | `#3730a3` | `#4338ca` |
| `--color-focus-ring` | `rgba(79,70,229,.42)` | `rgba(129,140,248,.50)` |

**Accent usage rules:**
- Light theme selected/active text: `accent-600`
- Dark theme selected/active text: **`accent-400`** (bright tier — 500/600 too dim on dark chrome)
- Focus ring: `--color-focus-ring`
- Left rail on active items (sidebar/palette): `accent-500`
- Accent is for focus ring / current selection / single primary action. Never large-area decoration.

### 3.3 Semantic Status

| Token | Light | Dark |
|---|---|---|
| `--color-status-success` | `#6dba78` | `#7ec98a` |
| `--color-status-warning` | `#c29a55` | `#d6a964` |
| `--color-status-danger` | `#d56a6a` | `#e07c7c` |
| `--color-status-info` | `var(--color-accent-500)` | `var(--color-accent-400)` |

`-soft` variants: ~14–18% opacity — **only for chip/pill/dot/1px border**, never whole-card wash. Dark soft variants use smaller area than light.

### 3.4 Data Palette

| Token | Light | Dark | Usage |
|---|---|---|---|
| `--color-data-primary` | `#6671db` | `#8d9bff` | Hero indigo — trend line, primary series |
| `--color-data-live-primary` | `#4f63f6` | `#a7b8ff` | Real-time throughput curve (brighter) |
| `--color-data-neutral` | `rgba(17,24,39,.30)` | `rgba(230,237,243,.28)` | Rankings bars, secondary series |
| `--color-data-secondary` | `#2f9eaa` | `#5fc7d4` | Teal — 3+ series only (low-frequency) |
| `--color-data-tertiary` | `#8b6fbf` | `#b29bdc` | Violet — 3+ series only (low-frequency) |
| `--color-data-attention` | `#d69554` | `#e6a865` | Transient/estimated data (not status warning) |

**Data rules:**
- Default: indigo + live + neutral. Teal/violet only when 3+ distinct series genuinely need them; prefer indigo luminance steps.
- Never auto-assign colors (no "model color").
- Live (data/telemetry) and success (system health) remain distinct — never interchange.
- Area fill ≤8% on charts; sourced from `-soft` tokens.

### 3.5 Material (Depth)

| Token | Light | Dark |
|---|---|---|
| `--material-glass-blur` | `8px` | `0px` |
| `--material-glass-blur-strong` | `8px` | `0px` |
| `--material-glass-blur-subtle` | `0px` | `0px` |
| `--material-shadow-card` | `0 2px 2px rgba(15,23,42,.04)` | `0 1px 2px rgba(0,0,0,.16)` |
| `--material-shadow-elevated` | Geist popover stack | Geist dark popover stack |
| `--material-overlay-scrim` | `rgba(17,24,39,.32)` | `rgba(0,0,0,.52)` |

**Shadow rules:**
- `shadow-card`: resting panels (border-first; shadow is optional — may be `none`).
- `shadow-elevated`: **floating layers only** — popover, dialog, drawer, menu, tooltip. Never on a resting panel.

### 3.6 Radius

| Token | Value | Applies to |
|---|---|---|
| `--radius-sm` | `6px` | Controls, chips, inputs, segmented controls, keycaps, sidebar items |
| `--radius-md` | `12px` | Cards, panels, popovers, menus |
| `--radius-lg` | `16px` | Dialogs, drawers, palette shell, page surface |
| `--radius-pill` | `999px` | Status pills (in-table), avatars, toggles |

**Exceptions:** Heatmap cells use literal `3px` (micro-grid, matches calendar reading). Radii `18/20/22/24/26/32` are **forbidden** in regular UI. A single view uses one radius family — no mixing 6/18/24/32.

### 3.7 Spacing

| Token | Value |
|---|---|
| `--space-section-gap` | `24px` | Top-level section rhythm in Overview |

Standard scale: `4 / 8 / 12 / 16 / 20 / 24 / 32 / 48`.

---

## 4. Component Contracts

### 4.1 Titlebar

- **Healthy default**: ONE calm chip — 26px high, `6px` rect (not pill), `--color-surface-subtle` background (NOT green), 1px `--color-border-subtle`, left 6px success-soft green dot (heartbeat, not fluorescent), text 12.5px/500/`--color-text-muted`. Label: `Live capture active` (narrow fallback `Capture active`).
- **No** mechanical telemetry string (`Service ready / queue 0 / lag 0ms`) in the titlebar.
- **Escalation**: degraded/reconnecting/backlog/lag-high → chip upgrades to warning in-place: `--color-status-warning-soft` background + 1px amber border + amber dot. Only genuinely blocking issues (service down) get a +1 danger auxiliary entry.
- **Popover** (click chip, ~280px): read-only — SERVICE section (readiness) + LIVE section (connection, queue depth, aggregate lag) + ACTIONS (View Activity, Open Settings). Opaque surface + `shadow-elevated` + 1px `border` + r12.
- **Right group**: page toolbar (refresh + range segmented control). `justify-content: space-between`.
- **No page title** in titlebar — page H1 lives in content area.
- Height 50px; left ~72px traffic-light drag region; `--color-chrome` background + bottom 1px `--color-border-subtle`.
- Status source: `shell.status` ONLY (via `deriveTitlebarStatus` view-model). No parallel health state machine. Popover is read-only — no `Restart`/`Diagnostics` actions.

### 4.2 Sidebar

- **No branding** at top. Padding top: `18→12px`. Pure directory from first group.
- **Groups**: `MONITORING` (Overview, Usage) / `TOOLS` (Prompt Palette) / `SYSTEM` (Settings). Labels: uppercase 11px `--color-text-faint`. Orphan items get no label.
- **Item**: height `32px` (was 36), padding `0 12px`, r6, icon 16px/stroke 1.75.
  - **Rest**: text + icon `--color-text-muted`, transparent background.
  - **Hover**: background `--color-hover`, no border/shadow.
  - **Active**: accent text + icon (light `accent-600` / dark `accent-400`) weight 500 + **2px left inset accent vertical bar** + very subtle neutral support (`--color-hover-strong`). **No accent-tinted block**.
  - **Focus-visible**: 2px `--color-focus-ring` inset outline.
- Container: `--color-chrome` background + right 1px `--color-border-subtle`.

### 4.3 Metric Cards

- **Default (including success)**: No wash, no top-accent, no dot, no shadow — only 1px `--color-border-subtle`. Opaque `--color-surface`, r12, padding 16/18. Number: 28px ~600 `--color-text` `tabular-nums` (**always neutral**). **No `--success` visual variant** (success = neutral).
- **Helper**: Default `--color-text-muted`; only very short status word/dot carries semantic color — never the whole line.
- **Exceptions** (never whole-card wash):
  - Warning: 2px **top flag** (amber, full-width/flush-top) + label-adjacent 6px amber dot.
  - Danger: 2px top red flag + **1px border changes to red** (semantic container tier).
  - Number and background **never change color**.
- **Ratios**: Top-level label `11px` / value `28px` / helper `12px`. Secondary (breakdown/detail) label `11px` / value `20px` / helper `11–12px`. Nested metrics always quieter than top-level.
- Grid: 3 columns, gap `12px` (was 14).

### 4.4 Overview

- **Page shell**: `.overview-console` uses `--space-section-gap: 24px` between top-level blocks. Content `max-width: 1600px` centered. Horizontal margin `24px`.
- **Section panels** (all r12, no `shadow-elevated`):
  - **Tier A primary** (Usage Trend, Real-time Throughput, Heatmap) = `--color-surface` + `--color-border` (strong) + no shadow (border-first).
  - **Tier B summary** (metric row) = `--color-surface` + `--color-border-subtle` + no shadow.
  - **Tier C supporting** (rankings, recent activity) = `--color-surface-subtle` + `--color-border-subtle` + no shadow.
- **In-panel emphasis**: title (16/600/text, top-left, bottom 1px `border-subtle` separating header/body) → data (strongest contrast/maximum area) → aux (total/legend/summary, smaller muted, header-trailing or footer only — never a second visual center).
- **State-in-frame**:
  - Loading: skeleton inside the panel frame (chart → low-contrast curve skeleton, table → skeleton rows, metrics → placeholder number boxes). Fallback: single `Loading…` line.
  - Error: inside the panel frame — one line error + inline `Retry` (tertiary). Copy: `Could not load usage data.` + `Retry`.
  - Empty: inside the panel frame — empty-state prompt + first action.
  - Degraded (page-level non-blocking): top thin **ribbon** (amber dot + one line + optional action). NOT a centered `PageState` card.
  - Catastrophic (summary completely unavailable): the one allowed full-page `PageState` replacement (restyled per new contract, no heavy shadow).

### 4.5 Charts

- **Line**: Trend `--color-data-primary` 1.75px; real-time `--color-data-live-primary` 2px + right-end 4px current-value dot (end-position locator, no halo/glow/pulse). Stroke **must** consume a chart token — never fall back to black/near-black.
- **Fill**: ≤8% at top, gradient to 0% at bottom.
- **Grid**: 3–4 horizontal thin lines `--color-border-subtle`. **Vertical grid disabled**.
- **Axis**: Line removed or `border-subtle`; ticks 11px `--color-text-faint`.
- **Baseline/target line**: 1px dashed `--color-border` (neutral) or amber (threshold only).
- **Tooltip**: Opaque `--color-surface` + `shadow-elevated` + 1px `border` + r6; label 12/600 + value 11 muted `tabular-nums`.
- **Multi-series**: Primary indigo / secondary neutral gray / third teal or violet (prefer indigo luminance steps).
- **Heatmap**: Empty = neutral substrate (light `#EDF0F3` / dark `#202732`); L1–L4 = discrete indigo steps (light darkens / dark brightens); cell 13px r3 (exception); **legend fixed 5 cells** (empty+L1–L4), never collapses in sparse mode.
- **Rankings**: Bar default **neutral gray** (`--color-data-neutral` ~.10); only **#1 leader row** gets indigo accent (~.14); value `tabular-nums` `--color-text`; container Tier C (`surface-subtle` + `border-subtle` + no shadow + r12).

### 4.6 Prompt Palette

- **Shell**: r**32→16**, opaque `--color-surface` + 1px `border` + `shadow-elevated`. Backdrop: scrim only (no radial top-glow). Window form also r16, no shadow.
- **Row**: min-h 44, r**14→6**. Hover: `--color-hover`. **Selected = neutral lift** (`--color-hover-strong`) + 2px left accent bar + title stays neutral high-contrast. Main identification is from background tier + position, not color; accent only as left rail / minimal cue.
- **Accessory denoise**: Default shows only essential metadata; hover reveals more affordance. Pin: neutral `◇` glyph / `PIN` mini-label (`--color-text-faint`) — **not success green**. Tags: `--color-text-muted` 12px, max 2 + overflow. Pin/tags/recent **never semantic color**.
- **Keycap / close**: No `box-shadow-card`. Keep 2px bottom border for physical key feel. r6. Footer hints: command reference, not a toolbar — text two steps weaker than list, keycaps for learnability only (not a button-group visual center).
- **⌘K menu**: r**16→12**, opaque + border + `shadow-elevated`. Item r6, hover `--color-hover`.
- **Shared grammar**: `PromptPaletteOverlay` / `PromptPaletteOverlayController` / `PromptPaletteWindowApp` / `PromptPalettePage` — all 4 carriers share the same row/selected/hover/accessory grammar. Only density and action organization differ.

---

## 5. Usage Rules (15)

1. Content area — no `backdrop-filter` (only `.desktop-titlebar` / `.desktop-sidebar` / modal scrim are exceptions).
2. Semantic `-soft` tints never color an entire card/panel — only chip/pill/dot/1px border.
3. Resting surface = opaque surface + 1px `border-subtle` + (optional) minimal card shadow. **Floating** layers (popover/dialog/drawer/menu/tooltip) only use `shadow-elevated`. Resting card: **border first, shadow optional** (may be 0).
4. Hierarchy = surface 2 tiers + border strength + spacing. Not blur/heavy shadows.
5. Single view uses one radius family (card=12, control=6). Forbidden: 18/20/22/24/32 mixed in.
6. Accent only for focus ring / current selection / single primary action. No large-area decoration.
7. Numbers are the metric card's primary visual; the card doesn't compete with them.
8. Content/panel surfaces always opaque (dark: no translucent content).
9. Data colors: default indigo + live + neutral. Teal/violet only when 3+ series genuinely need them; prefer indigo luminance steps.
10. Dark shadows use black, shorter (Geist dark scale). Resting surface: border-first, may be 0 shadow.
11. Accent text/selection: light uses `accent-600`, dark uses **bright tier `accent-400`**. Mid 500/600 not for dark text.
12. Dark segmented/toggle/selected control: no large high-saturation blocks. Only bright-tier accent text + thin border + very low alpha support.
13. Dark borders = structural cues, not decorative outlines. Don't let multiple panels in one view all have prominent borders simultaneously.
14. Live (data/telemetry) and success (system health) remain distinct in dark — no semantic interchange.
15. Status-soft in dark: only dot/pill/border, area even smaller than light.

---

## 6. Review Checklist

> Check these on every visual-change PR. Items marked `[G]` are guard-enforced; items marked `[T]` are test-enforced.

### Material
- [ ] [G] Content surfaces are opaque (no translucent `rgba` on `--color-surface`/`--color-surface-subtle`).
- [ ] [G] `backdrop-filter` only on chrome/modal selectors.
- [ ] [G] `--material-shadow-elevated` only on floating layers (popover/dialog/drawer/menu/tooltip), never on resting panels.
- [ ] [G] No raw hex colors in CSS consumer files (consume a token).

### Tokens
- [ ] [G] No stale/removed token names (`--color-surface-strong`, `--color-surface-elevated`, `--color-canvas-subtle`, `--color-border-soft`, `--color-sidebar`, `--radius-xs`, `--radius-xl`).
- [ ] [G] No radius outliers (18/20/22/24/26/32) — use `--radius-sm/md/lg`.
- [ ] [T] Light theme: `--color-surface: #FFFFFF`, `--color-canvas: #F4F5F7`, `--color-text: #1A1D23`.
- [ ] [T] Dark theme: `--color-surface: #171C24`, `--color-chrome: rgba(22,27,34,.96)`, blur stays 0.

### Components
- [ ] [G] `metric-card--success` CSS class does not exist (success renders as neutral).
- [ ] [G] Dark theme accent text uses `--color-accent-400`, not `--color-accent-500`/`--color-accent-600`.
- [ ] Titlebar shows exactly one calm chip when healthy; no capsule-stack.
- [ ] Sidebar active item = left rail + neutral support; no accent-tinted block.
- [ ] Metric cards: numbers always neutral; warnings/danger are top-flag only, never whole-card wash.
- [ ] Overview panels use correct tier border (Tier A = `--color-border`, Tier B/C = `--color-border-subtle`).
- [ ] Charts: no vertical grid; fill ≤8%; explicit chart-token stroke (never bare black).
- [ ] Prompt Palette: selected row = `--color-hover-strong` + left rail; pin is neutral (not green).

### Dark Theme
- [ ] Dark surfaces are opaque (no translucent content).
- [ ] Dark accent text = `accent-400` bright tier.
- [ ] Dark status-soft: only dot/pill/border, small area.
- [ ] Dark borders: structural only, not decorative.

---

## 7. Do / Don't

### Surfaces

| Do | Don't |
|---|---|
| Neutral surface + subtle border | Accent-tinted full card |
| Border-first resting panel, shadow optional | `shadow-elevated` on a resting card |
| Opaque content surfaces | Translucent/glass content surfaces |

### Titlebar

| Do | Don't |
|---|---|
| One calm chip in titlebar (healthy) | A row of telemetry capsules |
| Escalate the single chip in-place | Add new chips for each condition |
| Read-only popover (detail + existing nav) | Invent new backend actions in the popover |

### Sidebar

| Do | Don't |
|---|---|
| Selected = accent text + left rail + neutral lift | Selected = accent-tinted full block |
| Hover = `--color-hover` subtle lift | Hover = border/shadow/glow |

### Metrics

| Do | Don't |
|---|---|
| Numbers always neutral, always central | Green success cards, colored numbers |
| Warning/danger = 2px top flag only | Whole-card status-soft wash |

### Charts

| Do | Don't |
|---|---|
| Single indigo line + ≤8% fill | Multi-color glow chart |
| Rankings: neutral bars, one accent leader | Multi-row constant accent bars |
| Heatmap: fixed 5-cell legend | Dynamic legend that shrinks in sparse mode |

### Dark Theme

| Do | Don't |
|---|---|
| Dark accent text in `accent-400` | Dark accent text in `accent-500`/`600` |
| Restrained, structural borders | Glowing or decorative borders |
| Small-area status-soft | Large status-soft fills |

---

## 8. Sync List

| File | Role | Syncs with |
|---|---|---|
| `DESIGN-SYSTEM.md` | Canonical visual contract | ↔ spec, ↔ `tokens.css` |
| `apps/gui/src/styles/tokens.css` | Executable contract | ↔ `tokens.test.ts`, ↔ this document |
| `apps/gui/src/styles/tokens.test.ts` | Contract guard | Token existence + key values + usage rules |
| `apps/gui/src/styles/pages.css` | Consumer layer | Only consume tokens; no bare hex (whitelist-excepted) |
| `scripts/check-busytok-gui-surfaces.sh` | Regression guard | Stale tokens, blur, radius, hex, shadow, success class, dark accent |
| `DESIGN.md` | Narrative architecture overview | Non-normative; defers to this document for visuals |
```

- [ ] **Step 2: Commit**

```bash
git add DESIGN-SYSTEM.md
git commit -m "docs: canonical visual contract — DESIGN-SYSTEM.md

Creates the single source of truth for the Busytok visual design system:
4 governance principles, material contract, token reference (7 sub-sections),
6 component contracts, 15 usage rules, Review Checklist, Do/Don't tables,
and sync list. Co-evolves with tokens.css (executable contract) and the
Phase 1/2 guard rules. Phase 3."
```

---

### Task 2: Delete `THEME.md` + rewrite `DESIGN.md` visual section

**Files:**
- Delete: `THEME.md`
- Modify: `DESIGN.md:136-141`

**Interfaces:**
- Consumes: `DESIGN-SYSTEM.md` (Task 1) as the pointer target.
- Produces: `DESIGN.md` "Visual design" section → non-normative pointer. `THEME.md` gone. Zero references to Sentri system remain in repo docs.

- [ ] **Step 1: Delete `THEME.md`**

```bash
git rm THEME.md
```

- [ ] **Step 2: Rewrite the "Visual design" section in `DESIGN.md`**

In `DESIGN.md`, replace lines 136–141 (the entire "## Visual design" section):

**Old:**
```markdown
## Visual design

The visual design system ("Sentri Inspired") is documented in
[`THEME.md`](THEME.md) — a dark violet-and-lime design language
with a proprietary display sans, Rubik for UI copy, and Monaco
for code.
```

**New:**
```markdown
## Visual design

The visual design system (a calm, Geist-calibrated desktop audit-tool
aesthetic, indigo accent, SF Pro system font) is canonically documented in
[`DESIGN-SYSTEM.md`](DESIGN-SYSTEM.md). This section is non-normative for
visuals — the design-system document is authoritative.
```

- [ ] **Step 3: Verify no stale references to THEME.md or Sentri anywhere in repo docs**

```bash
rg -n 'THEME\.md|Sentri|violet-lime|display sans|Rubik.*UI copy|Monaco.*code' --glob '*.md' --glob '!target' --glob '!node_modules' --glob '!.git' --glob '!docs/superpowers/**'
```

Expected: no output (`docs/superpowers/` is excluded — both the spec and this plan contain Sentri/THEME.md references that are intentional historical/planning artifacts). If any other doc references THEME.md, update it to point to `DESIGN-SYSTEM.md`.

- [ ] **Step 4: Commit**

```bash
git add -u THEME.md DESIGN.md
git commit -m "docs: delete THEME.md, rewrite DESIGN.md visual section to pointer

THEME.md (551 lines, 100% abandoned Sentri system: violet-lime palette,
Rubik/Monaco fonts, proprietary display sans) is deleted — no salvage value.
DESIGN.md 'Visual design' section now points to DESIGN-SYSTEM.md as the
canonical visual contract. Phase 3."
```

---

### Task 3: Clean stale CSS comment

**Files:**
- Modify: `apps/gui/src/styles/components.css:311`

**Interfaces:** none new. The comment is semantically correct but its `per spec` back-reference is ambiguous post-spec-landing.

- [ ] **Step 1: Reword the comment**

In `apps/gui/src/styles/components.css`, change line 311:

**Old:**
```css
/* Dense data surface — selection uses surface-ladder elevation,
   not accent, per spec. */
```

**New:**
```css
/* Dense data surface — selection uses surface-ladder elevation
   (--color-hover-strong), not accent tint, per the design system
   material contract: semantic color marks status, not structure. */
```

- [ ] **Step 2: Verify no other stale-spec back-references remain in consumer CSS**

```bash
rg -n 'per spec' apps/gui/src/styles/components.css apps/gui/src/styles/pages.css apps/gui/src/styles/surfaces.css
```

Expected: exactly 1 hit (the one just reworded in components.css:311). Note: `tokens.css:201` also has a `per spec` comment, but that is the executable token contract tracking the spec — it is intentionally kept and out of scope for this cleanup. If >1, inspect each in the consumer CSS files — any that reference the old spec should be reworded to reference the design system document or describe the rule directly.

- [ ] **Step 3: Run guard to confirm clean**

```bash
pnpm check:gui-surfaces
```

Expected: exit 0 (the comment change is cosmetic and doesn't affect any guard rule).

- [ ] **Step 4: Commit**

```bash
git add apps/gui/src/styles/components.css
git commit -m "docs: reword stale 'per spec' CSS comment to reference design system

The single remaining 'per spec' comment in components.css:311 now references
the design system material contract by name, making the back-reference
traceable. No visual change. Phase 3."
```

---

### Task 4: Extend guard — metric-card--success + dark accent text

**Files:**
- Modify: `scripts/check-busytok-gui-surfaces.sh`

**Interfaces:**
- Consumes: Phase 1 guard rules (all kept). Task 1 `DESIGN-SYSTEM.md` existence.
- Produces: two new guard rules that enforce spec §8.3 items previously covered only by convention: (1) `metric-card--success` CSS class must not be defined, (2) dark-theme accent text must use `accent-400`, not `500`/`600`.

- [ ] **Step 1: Write the new guard rules**

In `scripts/check-busytok-gui-surfaces.sh`, append after the raw-hex block (after `fi` on line 85):

```bash
# ── Geist refactor Phase 3: metric-card--success CSS must not exist ──
# Spec §6.3 + Phase 2 Task 4: success renders as neutral; the old
# --success visual variant CSS rule is deleted. This scans CSS consumer
# files only — TS test assertions that check the class is absent (e.g.
# OverviewPanels.test.tsx .includes("metric-card--success")) are valid
# and intentionally out of scope.
if rg -n 'metric-card--success' apps/gui/src/styles --glob '*.css'; then
  echo "metric-card--success CSS rule found — success renders as neutral (spec §6.3)"
  exit 1
fi

# ── Geist refactor Phase 3: dark accent text uses accent-400 ─────────
# Spec §4.2 rule + §5 rule 11 + §7: dark-theme selected/active accent
# text must use the bright tier (--color-accent-400), not 500/600 which
# are too dim on dark chrome. Scan for CSS rules under a dark-theme
# selector whose color property references accent-500 or accent-600.
# The awk tracks brace-nesting depth and detects :root[data-theme="dark"]
# on the same line as the opening brace (the project's only dark-theme
# selector form). When darkDepth is set, any `color: var(--color-accent-500|600)`
# inside that block or its nested children is a violation.
# Single-line CSS rules (e.g. `selector { prop: val; }`) are skipped
# entirely in the closing-brace handler to avoid depth drift — they
# contribute net-zero to the brace stack.
if ! awk '
  BEGIN { depth = 0; darkDepth = 0; bad = 0 }
  /\}/ {
    # Single-line CSS rule: { and } on same line → net zero depth change.
    if ($0 ~ /\{/) { next }
    if (darkDepth > 0 && depth == darkDepth) darkDepth = 0
    depth--
    next
  }
  /\{/ { depth++; if ($0 ~ /:root\[data-theme="dark"\]/) darkDepth = depth; next }
  darkDepth > 0 && depth >= darkDepth && /color:.*var\(--color-accent-500\)|color:.*var\(--color-accent-600\)/ {
    print FILENAME ":" NR ": dark accent text uses " substr($0, index($0, "accent-")) " instead of --color-accent-400"; bad = 1
  }
  END { exit bad ? 1 : 0 }
' apps/gui/src/styles/components.css apps/gui/src/styles/pages.css; then
  echo "Dark theme accent text must use --color-accent-400 (bright tier), not 500/600"
  exit 1
fi

# ── Geist refactor Phase 3: DESIGN-SYSTEM.md must exist ──────────────
if ! test -f DESIGN-SYSTEM.md; then
  echo "DESIGN-SYSTEM.md (canonical visual contract) not found at repo root"
  exit 1
fi

# ── Geist refactor Phase 3: THEME.md must NOT exist ──────────────────
if test -f THEME.md; then
  echo "THEME.md still exists — was deleted in Phase 3 (abandoned Sentri system)"
  exit 1
fi
```

- [ ] **Step 2: Run guard to verify current state**

```bash
pnpm check:gui-surfaces
```

Expected: exit 0. The guard's `rg 'metric-card--success' apps/gui/src/styles --glob '*.css'` is scoped to CSS consumer files only — the `OverviewPanels.test.tsx` negative assertion `c.className.includes("metric-card--success")` (which correctly verifies the class is absent on DOM elements) is a `.tsx` file and intentionally out of scope.

- [ ] **Step 3: Verify dark accent-400 awk rule in isolation**

The awk rule tracks brace-nesting depth and detects `:root[data-theme="dark"]` on the same line as the opening brace. It flags any `color: var(--color-accent-500|600)` at or below that depth. Current state: components.css:56 has `:root[data-theme="dark"] .desktop-sidebar__item.is-active { color: var(--color-accent-400); }` — this uses 400, which is correct. The awk rule matches on 500/600, so it must pass cleanly.

Run the awk in isolation to confirm:

```bash
awk '
  BEGIN { depth = 0; darkDepth = 0; bad = 0 }
  /\}/ { if ($0 ~ /\{/) { next }; if (darkDepth > 0 && depth == darkDepth) darkDepth = 0; depth--; next }
  /\{/ { depth++; if ($0 ~ /:root\[data-theme="dark"\]/) darkDepth = depth; next }
  darkDepth > 0 && depth >= darkDepth && /color:.*var\(--color-accent-500\)|color:.*var\(--color-accent-600\)/ {
    print FILENAME ":" NR ": " $0; bad = 1
  }
  END { exit bad ? 1 : 0 }
' apps/gui/src/styles/components.css apps/gui/src/styles/pages.css
```

Expected: exit 0, no output.

- [ ] **Step 4: Commit**

```bash
git add scripts/check-busytok-gui-surfaces.sh
git commit -m "test(gui): guard metric-card--success absence + dark accent-400 + doc existence

Extends check-busytok-gui-surfaces.sh with Phase 3 rules:
- metric-card--success CSS class must not be defined (success=neutral).
- Dark-theme accent text must use --color-accent-400 (bright tier), not
  500/600 which are too dim on dark chrome.
- DESIGN-SYSTEM.md must exist at repo root.
- THEME.md must NOT exist (deleted in Phase 3).
Phase 3."
```

---

### Task 5: Phase 3 verification gate

**Files:**
- Verify only (no edits unless a gate check fails).

- [ ] **Step 1: Full verification gate**

Run (all must pass):

```bash
pnpm --filter @busytok/gui typecheck
cd apps/gui && npx vitest run          # full suite — expect all green
pnpm coverage:gui                       # lines ≥90%
pnpm check:gui-surfaces                 # all guard rules green (Phase 1 + 2 + new Phase 3 rules)
pnpm --filter @busytok/gui build
```

Expected: typecheck clean; full suite PASS (~700+ tests); coverage ≥90% lines; guard exit 0 (all rules: stale surfaces, radius, shadow-elevated, stale tokens, backdrop-filter, raw hex, metric-card--success, dark accent-400, DESIGN-SYSTEM.md exists, THEME.md absent); production build succeeds.

- [ ] **Step 2: Manual spot-checks**

1. `DESIGN-SYSTEM.md` renders correctly on GitHub (`##` sections, tables, checkboxes, code blocks).
2. `DESIGN.md` "Visual design" section is a single pointer paragraph, no stale Sentri/Rubik/Monaco language.
3. `THEME.md` is gone from the repo root.
4. `apps/gui/src/styles/components.css` line 311 comment references "design system material contract."

- [ ] **Step 3: Commit (only if any fix was needed)**

If Steps 1–2 required no edits, this is a no-commit verification. If any fix was applied:

```bash
git commit -am "test(gui): Phase 3 verification gate green"
```

---

## Self-Review

**1. Spec coverage (Phase 3 = spec §8):**
- §8.1 new `DESIGN-SYSTEM.md` canonical contract → Task 1. ✓
- §8.1 `DESIGN.md` non-normative identity + pointer → Task 2. ✓
- §8.2 delete `THEME.md` (551-line Sentri system) → Task 2. ✓
- §8.2 sweep CSS inline `/* per spec */` comments → Task 3. ✓
- §8.2 clean dead tokens (verified none linger — Phase 1 guard already covers) → Task 5 verification. ✓
- §8.3 Review Checklist + automated guards → Task 1 (Checklist in DESIGN-SYSTEM.md) + Task 4 (new guard rules). ✓
- §8.4 Do/Don't tables → Task 1 (section 7 of DESIGN-SYSTEM.md). ✓
- §8.5 Sync list → Task 1 (section 8 of DESIGN-SYSTEM.md). ✓

**2. Placeholder scan:** None — every step has exact paths, code, commands, expected output.

**3. Type consistency:** `DESIGN-SYSTEM.md` path used in Task 1 (create), Task 2 (pointer from DESIGN.md), Task 4 (guard existence check). `metric-card--success` scoped to CSS files in guard (not TS test assertions). Dark accent-400 awk guard merges `:root[data-theme="dark"]` detection into the `/\{/` rule (avoiding the `next` shadowing bug where the standalone detection rule never fires when selector and `{` share a line). `--color-accent-400` dark rule matches spec §4.2 + §5 rule 11.

**4. Out of scope (explicit):**
- No component/token changes (Phase 1+2 already landed).
- No new toolchain (extends existing bash guard).
- No TS coverage changes (docs-only phase).
- The `OverviewPanels.test.tsx` negative assertion (`expect(successCards.length).toBe(0)`) is left as-is — it correctly verifies the class is absent; the guard's CSS-only scope avoids a false positive.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-23-busytok-geist-refactor-phase3-docs.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
