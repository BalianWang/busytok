# Receipt Share Image — Design

- **Date:** 2026-06-26
- **Branch:** `feat/receipt-share-image` (off `main`)
- **Status:** Draft (awaiting plan)
- **Scope:** MVP — daily receipt share image (HTML/CSS + DOM→PNG)

## 1. Goal & motivation

Busytok already computes daily token usage and estimated cost. This feature turns a
day's data into a **shareable "receipt" image** — a mall/restaurant-style consumption
slip — to drive word-of-mouth growth ("裂变"). The receipt is a share artifact, not app
UI: it is built to look beautiful when posted to 微信 / X / 小红书 / 朋友圈, and to
double as a privacy-trust signal ("local audit, nothing uploaded").

The share button lives in the top-right toolbar, immediately left of the existing
**Refresh** button, so that **Share and Refresh are always adjacent**, and the
conditional **Update** badge (when a new version is available) sits to the left of
Share.

## 2. Non-goals (MVP — YAGNI)

- **SVG + resvg/usvg fallback render path.** Deferred to Phase 4. The HTML/CSS +
  DOM-capture path is the only render path in MVP.
- **Weekly / monthly receipts.** MVP is daily only (today + a selectable past day).
- **Per-agent (Claude Code vs Codex) breakdown on the receipt.** Data exists; cut for
  MVP focus. Can be added later without schema changes.
- **Prompt-reuse metric.** Unrelated to token audit (belongs to Prompt Palette). Cut.
- **Localization / Chinese.** The app has no i18n today; the receipt is English-only,
  matching the app. (No CJK font needed → tiny font bundle.)
- **Multiple templates.** One polished "Receipt Classic" template. More templates are a
  later phase.
- **CLI export** (`busytok receipt export`). Comes with the Phase 4 SVG path.

## 3. Decisions (resolved forks)

| Decision | Resolution | Why |
|---|---|---|
| Visual language | **Own, standalone design language** — fully independent of the app's `DESIGN-SYSTEM.md` | A share image should read like a poster, not an app panel. The app's neutral/indigo restraint would undercut the "receipt" metaphor. |
| Fonts | **Bundle dedicated fonts** (JetBrains Mono + Geist, OFL) | (a) Receipt aesthetics; (b) load-bearing for export fidelity — DOM-capture libs break on non-same-origin web fonts, so bundling is the fix, not just a nicety. |
| Time scope | **Today + selectable past day** | "Today only" produces empty receipts in the morning; a `date` param on the backend is near-free and unlocks sharing any noteworthy past day. |
| Cost ($) treatment | **Secondary detail**, tokens are the hero | Cost is an estimate (`cost_status`) and $ is sensitivity-prone. Cost appears naturally as the per-line "price" column and in the TOTAL block, but is never the giant hero number. |
| Render library | **`modern-screenshot`** (SVG `foreignObject` approach) | Higher fidelity than `html2canvas` for gradients/masks (html2canvas re-implements rendering and has long-standing gradient bugs); actively maintained. |
| Data source | **New `receipt.daily` control method** (not GUI-side composition) | GUI never touches SQLite. `daily_usage` materializes the full token split per day; a few new queries (`session_count` via ms-window, `peak_hour`) + plumbing give a faithful ViewModel. |
| Save path | **dialog `save()` (path) + Rust `save_receipt_png` command (write)** | `tauri-plugin-dialog`'s `save()` returns **only a path** — it cannot write. A Rust `#[tauri::command]` does the actual `std::fs::write` (unified logging + typed errors). No `tauri-plugin-fs` → minimal capability surface. |

## 4. Architecture & data flow

The GUI must not read SQLite directly (architecture invariant). Data flows through the
existing control socket:

```
ShareReceiptButton (Overview toolbar)
  → ReceiptPreviewDialog (Radix Dialog)
    → useDailyReceipt(date)  [React Query]
        → invoke_busytok("receipt.daily", { date })  [busytokClient.ts]
            → Unix-domain control socket → busytok-service
            → supervisor::receipt_daily   (always reads daily_usage; ignores fast path)
            → store::read_daily_receipt(generation_id, rtz, date)
                ← reuses `daily_usage` (per-day materialized rollup, full token split)
            → ReceiptDailyDto
    → ReceiptViewModel (single source of truth)
        ├─ <ReceiptPaper vm={…}/> in the dialog (CSS-scaled to fit)     [preview]
        └─ <ReceiptPaper vm={…}/> off-screen root at 420 CSS px          [export target]
           (same component + same receipt.css → identical output)
  → export (capture the OFF-SCREEN instance):
      modern-screenshot domToBlob(off-screen root, { scale: 3 })
        → blob.arrayBuffer() → Uint8Array
        → writeImage(bytes)                 [copy — accepts raw PNG bytes]
        / dialog save() → invoke save_receipt_png(path, bytes)  [save — Rust writes]
        / writeText(summary)                [text fallback]
```

A DOM node lives in one place, so preview and export are **two renders of the same
`<ReceiptPaper>` component** driven by one `ReceiptViewModel` and one `receipt.css`. The
dialog renders its own scaled instance for live preview; the export captures a separate,
off-screen (not `display:none`) instance at exactly 420 CSS px. Shared data + style
contract keeps them pixel-identical; only the off-screen instance is captured.

## 5. `receipt.daily` — backend contract & wiring

**New control method:** `receipt.daily`

- **Request:** `ReceiptDailyRequestDto { date: Option<String> }` — `date` is
  `YYYY-MM-DD` in the **current reporting timezone** (server-resolved to today when
  `None`). See Data availability for the single-TZ caveat.
- **Response:** `ReceiptDailyDto` (below), wrapped in the existing `ReadEnvelopeDto`.

**`ReceiptDailyDto`** (ts-rs → `packages/busytok-protocol-types/src/generated.ts`):

```ts
interface ReceiptDailyDto {
  date: string;                  // "2026-06-26" (reporting TZ)
  date_label: string;            // "FRI · JUN 26, 2026" — format semantics match src/lib/formatters.ts, replicated server-side (so the future Phase 4 Rust render path can share the same ViewModel)
  timezone: string;
  metrics: {
    total_tokens: number;        // HERO
    input_tokens: number;        // free, from daily_usage
    output_tokens: number;       // free
    cache_read_tokens: number;        // free, daily_usage.cache_read_tokens
    cache_creation_tokens: number;    // free, daily_usage.cache_creation_tokens ("cache write")
    cache_hit_rate: number | null;    // derived: cache_read_tokens / (input_tokens + cache_read_tokens); null when denominator is 0
    cost_usd: number | null;          // secondary; nullable
    cost_status: "exact" | "partial" | "unavailable"; // DERIVED via NULL heuristic — daily_usage has no such column (see Data availability)
    event_count: number;         // requests
    session_count: number;       // NEW query: COUNT(DISTINCT session_id) over a reporting-TZ ms-window (NOT by date string)
    peak_hour: { label: string; tokens: number } | null; // NEW query over usage_buckets_hour + UTC→TZ hour relabel
  };
  top_models: { name: string; tokens: number; cost_usd: number | null; cost_status: "exact" | "partial" | "unavailable" }[]; // ranked by tokens desc; per-row cost_status (NULL heuristic)
  brand: { name: "BUSYTOK"; tagline: string; github: string; generated_at_ms: number };
}
```

**Data availability & sources of truth** (verified against the schema):

- **Hero metrics + per-model slices come from `daily_usage`** (`0001_baseline.sql:136-156`)
  — it is the **only** day rollup carrying the full token split
  (`input/output/total/cached_input/cache_creation/cache_read/reasoning/thoughts/tool` +
  `cost_usd` + `estimated_cost_usd` + `event_count`), keyed by
  `(date, timezone, agent, project_hash, model, generation_id)`. Per-model slices = `GROUP BY model`.
  `receipt_daily` therefore **always reads `daily_usage` regardless of `use_sql_fast_path`**
  — do NOT mirror `overview_summary`'s fast path, which reads the cache-less UTC bucket
  tables.
- **`daily_usage` is single-timezone-only.** Rows are tagged with the *current* reporting
  TZ, and a TZ change triggers `rebuild_rollups` that re-projects all historical days into
  the new TZ (`writer.rs:1343-1346`). So `date` is always expressed in the **current**
  reporting TZ; a selectable "past day" means "that calendar day as defined by my current
  TZ." Acceptable for the receipt — state it so the implementer does not try to honor a
  stored historical TZ.
- **Two things need genuinely new SQL (not reuse):**
  - `session_count` — **MANDATORY definition: `COUNT(DISTINCT session_id) FROM usage_events
    WHERE generation_id=? AND timestamp_ms >= start_ms AND timestamp_ms < end_ms`** over the
    reporting-TZ day's **ms-window** `[start_ms, end_ms)` (the same window `overview_summary`
    uses). Do NOT filter by the `date` string, and do **NOT** use
    `usage_by_session_day.last_active_at_ms` — that column is a UTC-day *last-active* stamp
    and silently drops sessions whose last activity fell in a neighboring UTC day even though
    they were active inside the reporting-TZ window. `usage_events` is the authoritative
    source. (If a day-windowed `usage_events` scan ever proves too slow, the fix is a **new
    dedicated aggregate** indexed by the reporting-TZ day — never a loose approximation over
    `last_active_at_ms`.)
  - `peak_hour` — a **new** query over `usage_buckets_hour` for the active generation over
    the reporting-TZ day's UTC ms-window, `SUM(tokens) GROUP BY bucket_start_ms`, convert
    each UTC `bucket_start_ms` to a reporting-TZ hour label, return the max. NOT a reuse of
    `read_overview_trend_hourly` (which returns per-`agent,model` UTC-keyed rows).
- **`cost_status` is derived, not stored**, on `daily_usage` (the column does not exist
  there). Produce it via the same NULL heuristic the IANA reader uses
  (`read_overview_summary_from_daily_usage` → `ui_models::cost_status`): a NULL rolled-up
  `cost_usd` → `"unavailable"`, else `"exact"`/`"partial"`. This applies at **two levels**:
  the day aggregate (`metrics.cost_status`) AND **each `top_models` row** — a model's
  per-row status is `partial`/`unavailable` if any of its contributing `daily_usage` rows
  has NULL cost. `"estimated"` is unreachable from `daily_usage` (one unknown-cost event
  NULLs the sum). Carrying per-row status is what lets the receipt render line-item cost
  honestly instead of dressing a partial amount up as exact.
- `cache_hit_rate` = `cache_read_tokens / (input_tokens + cache_read_tokens)`; `null` when
  the denominator is 0 → receipt omits the chip. (`daily_usage` has no
  `prompt_input_total_tokens`; this approximation uses confirmed columns. The unused
  `cached_input_tokens` column is not surfaced in MVP.)

**Required regression test:** a store unit test with a `+08:00` fixture asserting
`session_count` and the hero token totals describe the **same** set of events (guards the
UTC-vs-reporting-TZ basis bug).

**Wiring** (mirrors every existing read view — `overview.summary`, `overview.trend`):
1. `crates/busytok-protocol/src/methods.rs` — add `"receipt.daily"` to `method_manifest()`.
2. `crates/busytok-protocol/src/dto.rs` — `ReceiptDailyRequestDto`, `ReceiptDailyDto`,
   `ReceiptModelSliceDto`, sub-DTOs with `#[derive(TS)]`. **Then** append every new DTO to
   the hand-maintained `type_defs` vec in `crates/busytok-protocol/src/ts.rs:37`
   **in dependency order** (leaf structs first), and run
   `scripts/generate_protocol_types.sh` (→ `cargo test -p busytok-protocol
   generate_typescript_types`). Deriving `TS` alone does NOT surface types in
   `generated.ts` — this registration step is mandatory and easy to miss.
3. `crates/busytok-control/src/dispatch.rs` — add `receipt_daily` to the `RuntimeControl`
   trait + a match arm, **and** update the `Arc<T>` blanket impl (`dispatch.rs:946`) and
   `TestRuntimeControl` (`dispatch.rs:478`) by hand (neither is automatic).
4. `crates/busytok-runtime/src/supervisor.rs` — implement `receipt_daily`, reusing
   `range::parse_timezone` and `active_generation_id_from_snapshot` (`supervisor.rs:1104`),
   **always reading `daily_usage`** (see Data availability) regardless of
   `use_sql_fast_path`.
5. `crates/busytok-store/src/read_queries.rs` + `read_models.rs` —
   `read_daily_receipt(conn, generation_id, rtz, date) -> ReceiptDailyRow` (hero metrics +
   top-models from `daily_usage`), the `session_count` query (DISTINCT, ms-window), and the
   `peak_hour` query (hour buckets + TZ relabel).

**Privacy:** every field is audit metadata already surfaced by `overview.*` /
`breakdown.*` / `activity.*`. No prompt bodies, no new privacy surface.

## 6. Receipt visual design language (standalone)

Intentionally divergent from `DESIGN-SYSTEM.md`. Target: a premium mall/restaurant
consumption receipt.

**Paper & material**
- Warm thermal paper, cream gradient `#FFFDF6 → #F6EFE2` (never pure white), with a
  faint SVG `feTurbulence` paper-grain noise (~10% opacity, `multiply`).
- Paper floats on a soft warm-neutral stage (`#E9E4DA` + faint vignette) with layered
  soft shadows — gives the "floating slip" look in the shared image.
- Top & bottom **tear edges**: inline SVG mask, irregular torn path (signature receipt
  cue; more refined than CSS zig-zag).

**Typography (bundled woff2, OFL, Latin subset — tiny)**
- Numbers & line items: **JetBrains Mono** (OFL) — thermal-print feel, `tabular-nums`.
- Brand header: **Geist** (OFL) bold uppercase + tracking — ties to the project's
  "Geist-calibrated" identity, contrasts the mono body.
- `await document.fonts.ready` before every capture.
- Font `@font-face` scoped to `.receipt-paper` so app chrome (system fonts) is untouched.

**Palette (receipt-only)**
- Ink `#211B14` (warm near-black); secondary label `#6B5E4A` (warm gray-brown).
- Single accent: **oxide red `#B4452F`** (like a "paid" chop) — used only on the stamp,
  the TOTAL double-rule, and the brand underline. No indigo.

**Layout (top → bottom)**
1. Tear edge (top).
2. **Header** — `BUSYTOK` (Geist bold tracked) / `AI CODING · TOKEN RECEIPT` (mono faint).
3. Dashed divider.
4. **Meta** — date label, peak hour, faux receipt serial (`#0626-A3F2`, date-derived),
   timezone. Mono.
5. **Hero** — total tokens (large mono) + `TOTAL TOKENS` label; one secondary line:
   `in X · out Y · cache Z` and `est. $cost · cache hit N%`.
6. Dashed divider.
7. **Items** — per-model rows: `model name` (left, ellipsis) · leader dots · `tokens` ·
   `cost` (right, `tabular-nums`). Header `ITEMS`. **Cost honesty:** render per-line cost
   from the row's `cost_status` — exact as `$24.10`, partial as `≈$24.10`, unavailable as
   `—`. Never show a partial amount as if exact.
8. **TOTAL block** — double rule + bold: `TOTAL   <total tokens> tok   $<total cost>`.
9. **Stamp** — rotated (~−12°) oxide-red `LOCAL AUDIT` chop, semi-transparent.
10. **Footer** — `NO PROMPTS UPLOADED` / `LOCAL AUDIT ONLY` /
    `generated by busytok · /busytok`, plus a faux barcode strip (pure decoration).

**Layout rules**
- **Width fixed, height auto:** 420 CSS px wide; height flows with model count (no fixed
  900 px). Export at **3× → 1260 px wide PNG**.
- **Items truncation:** list Top 5 models by tokens; if more, collapse the rest into one
  `OTHERS (n)` row (aggregated tokens + cost). Keeps the receipt tidy for heavy days.
- **Two foci only:** the hero number and the TOTAL block. Everything else is muted.
- Per-line item shows **total tokens + cost** (not the in/out/cache split — that lives at
  the summary level only). Per-line cost follows the row's `cost_status` (`$` / `≈$` / `—`);
  the TOTAL block shows the aggregate cost and its `cost_status`.

**ASCII sketch**

```
   ░▒▓ tear edge ▓▒░
 ┌────────────────────────────────────┐
 │          B U S Y T O K             │
 │      AI CODING · TOKEN RECEIPT     │
 │  ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─    │
 │  FRI · JUN 26, 2026   PEAK 14:00   │
 │  RECEIPT #0626-A3F2   TZ Asia/Sh   │
 │  ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─    │
 │          3,412,888                 │  ← hero: total tokens
 │           TOTAL TOKENS             │
 │   in 2.1M · out 0.9M · cache 1.8M  │
 │   est. $47.21  ·  cache hit 68%    │
 │  ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─    │
 │  ITEMS                             │
 │  claude-sonnet-4-5                 │
 │  ·············· 1,820,442  $24.10  │
 │  claude-haiku-4-5                 │
 │  ··············   810,200   $1.55  │
 │  gpt-5.1  ··············· 530,000 $18.40
 │  deepseek-v3.2 ············ 252,246  $3.16
 │  OTHERS (3) ···············  40,000  $0.00
 │  ═══════════════════════════════   │
 │  TOTAL    3,412,888 tok   $47.21   │  ← TOTAL block
 │            ◎ LOCAL AUDIT           │  ← oxide-red stamp
 │  NO PROMPTS UPLOADED               │
 │  LOCAL AUDIT ONLY                  │
 │  generated by busytok · /busytok   │
 │  ▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌  │
   ░▒▓ tear edge ▓▒░
```

## 7. Export / share flow & button placement

**Button placement** (no special-casing — satisfies the adjacency requirement for free)
- Register a **dedicated Overview toolbar** in `OverviewPage.tsx` via
  `useRegisterPageToolbar(<><ShareReceiptButton /><RefreshButton /></>)`, replacing the
  current `useRefreshToolbar()` call (`OverviewPage.tsx:150`). (`useRefreshToolbar` returns
  `void` and registers `<RefreshButton>` internally — there is no return value to "wrap", so
  a dedicated `useRegisterPageToolbar` composing both buttons is the real seam.)
- `AppShell.tsx` renders `<UpdateBadgeButton /> {toolbarContext?.toolbar ?? null}`
  (`AppShell.tsx:84-85`); the toolbar slot is `null` until a page registers one, so the
  visual order `Update | Share | Refresh` holds **only on the Overview page** (the intent —
  Share is the daily-receipt entry point). Share is always adjacent to Refresh; Update
  lands left of Share only when an update is available.
- Styling reuses the refresh-button spec (28×28 pill, `--color-border-subtle`,
  `--color-surface`; CSS at `components.css:1256-1304`); icon via `lucide-react`
  (`Share` / `Receipt`, confirmed importable). Visible on the Overview page only.

**Preview dialog** (Radix Dialog — already a dependency)
- Live `<ReceiptPaper>` preview, scaled to fit.
- **Date control** (default today; switch to any day with data — drives the `date` param).
  There is **no existing date-picker component** in the GUI, so MVP uses a native
  `<input type="date">` (simplest) or a small prev/next-day chevron + label — budget ~one
  small component. Empty day → calm empty state, export actions disabled.
- **Action row:**
  - **Copy image** (primary): `domToBlob(off-screen root, { scale: 3 })` →
    `blob.arrayBuffer()` → `Uint8Array` → `writeImage(bytes)` (the installed
    `@tauri-apps/plugin-clipboard-manager@2.3.2` accepts raw bytes directly — no `Image`
    wrapper needed). Toast "Copied".
  - **Save PNG**: `tauri-plugin-dialog` `save()` returns a path →
    `invoke("save_receipt_png", { path, bytes })` (Rust `std::fs::write` + tracing + typed
    error). `save()` does NOT write — the command does.
  - **Copy text summary** (fallback): short summary via existing text clipboard.
- Capture failure → toast error, degrade to text summary.

## 8. Infrastructure additions (concrete)

1. **`apps/gui/package.json`** — add `modern-screenshot`.
2. **Fonts** — create `apps/gui/public/fonts/` (the dir does not exist yet; the project
   bundles zero fonts today) and add `JetBrainsMono-*.woff2` + `Geist-*.woff2` (OFL, Latin
   subset). `@font-face` in a new `receipt.css`, scoped under `.receipt-paper`.
3. **Save (two parts — the link must close):**
   - `tauri-plugin-dialog` (Cargo + `lib.rs` registration + `@tauri-apps/plugin-dialog` JS
     dep + capability `dialog:allow-save`) — `save()` returns **only the path**; it does
     NOT write the file.
   - A new Rust command `#[tauri::command] save_receipt_png(path: String, bytes: Vec<u8>)`
     in `apps/gui/src-tauri/src/commands.rs`, registered in `generate_handler!`
     (`lib.rs:622`). It does `std::fs::write`, logs via `tracing`, returns a typed error.
     (Chosen over `tauri-plugin-fs` to keep the capability surface minimal — no arbitrary
     file-write exposure.)
4. **Clipboard image** — add `clipboard-manager:allow-write-image` to
   `src-tauri/capabilities/default.json` (the plugin itself is already registered).
5. **Backend `receipt.daily`** — per §5.
6. **New GUI module** `apps/gui/src/features/receipt/` — `ReceiptPaper.tsx`,
   `ReceiptPreviewDialog.tsx`, `ShareReceiptButton.tsx`, `useReceiptExport.ts`,
   `receipt.css`, and mock-data fixtures.

### Design-system guard — `receipt.css` must be excluded (NO whitelist exists)

`scripts/check-busytok-gui-surfaces.sh` has **no whitelist mechanism** (and there are no
"whitelist-excepted consumer surfaces" — the only exclusion today is `tokens.css`). It
runs three `rg` passes over CSS: bare-hex (`apps/gui/src/styles` excl. `tokens.css`,
~line 82), radius-outlier (`apps/gui/src`, ~line 19), and stale-token-name
(`apps/gui/src`, ~lines 46-47). The receipt **intentionally** uses its own hex palette and
non-standard radii (standalone design language), so it WILL trip all three.

The implementor MUST extend `check-busytok-gui-surfaces.sh` to exclude **only the receipt
stylesheet file** — add `--glob '!**/receipt.css'` to **each** of the three `rg`
invocations. Do **NOT** exclude the whole `features/receipt/**` subtree: the radius and
stale-token guards scan `apps/gui/src`, so a directory-level glob would silently exempt that
feature's `.ts`/`.tsx` too, weakening the guard. File-level exclusion (`!**/receipt.css`)
is precise — the guards already scope to `*.css`, so only the one stylesheet is let through.

## 9. Testing

- **Rust (`busytok-store`):** unit test for `read_daily_receipt` — field completeness,
  cache split, `session_count`, Top-5 + `OTHERS` aggregation, empty-day handling, **plus a
  `+08:00` fixture asserting `session_count` and hero token totals describe the same
  events** (guards the UTC-vs-reporting-TZ basis bug). Mirror existing store test style;
  keep workspace coverage ≥ 85%.
- **Vitest (jsdom):**
  - `ReceiptPaper` across 6 mock fixtures — `normal_day`, `heavy_day`, `zero_cost`,
    `long_model_name`, `many_models` (assert `OTHERS` row), `no_data` (empty state).
    Structural asserts: number formatting, truncation, stamp/footer presence.
  - `useReceiptExport` logic with mocked capture/clipboard/dialog — assert call order
    `document.fonts.ready → domToBlob → writeImage(bytes)`, the save path, and the
    text-summary fallback.
  - ViewModel formatting (reuse `src/lib/formatters.ts`).
- **E2E (Playwright):** local/optional only — Playwright is not run on CI, and **Vitest**
  hangs on CI runners (`CONTRIBUTING-WORKFLOW.md:46`). Acceptance via
  `scripts/verify_acceptance.sh` + manual PNG inspection.
- **Dev harness:** mock-data fixtures + a dev-only toggle (env-gated) in the preview
  dialog to switch real ↔ mock datasets, since the app has no router for a `/receipt-lab`
  route.

## 10. Phasing (for the implementation plan)

1. **Backend** — `receipt.daily`: store query → read model → protocol DTO → control
   dispatch → supervisor impl + Rust unit test.
2. **Receipt component** — `ReceiptPaper` + `receipt.css` + bundled fonts + mock
   fixtures + Vitest tests across fixtures.
3. **Export** — `useReceiptExport` (capture / copy / save / text fallback) + clipboard &
   dialog capabilities.
4. **Integration** — `ReceiptPreviewDialog` + `ShareReceiptButton` in the Overview
   toolbar; extend the design-system guard to exclude `receipt.css`; acceptance.

## 11. Future (out of MVP)

Phase 4: SVG + `resvg`/`usvg` fallback render path; CLI `busytok receipt export`;
weekly/monthly denominations; additional templates; per-agent breakdown.

## 12. Sources

- [Capturing DOM as Image Is Harder Than You Think — monday.com engineering](https://engineering.monday.com/capturing-dom-as-image-is-harder-than-you-think-how-we-solved-it-at-monday-com/)
- [modern-screenshot (npm)](https://www.npmjs.com/package/modern-screenshot) · [web-font issue #16](https://github.com/qq15725/modern-screenshot/issues/16)
- [html2canvas gradient bug (Stack Overflow)](https://stackoverflow.com/questions/62443267/calculated-gradient-in-html2canvas-doesnt-work-properly)
- [Tauri clipboard-manager JS API (writeImage)](https://v2.tauri.app/reference/javascript/clipboard-manager/) · [Image namespace (fromBytes)](https://v2.tauri.app/reference/javascript/api/namespaceimage/)
- ["expected RGBA image data" gotcha — plugins-workspace#1463](https://github.com/tauri-apps/plugins-workspace/issues/1463)
