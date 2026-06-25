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
  - **Rest**: text + icon `--color-nav-text` (between text and muted — readable primary-nav rest state, distinct from settings secondary copy). Transparent background.
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
