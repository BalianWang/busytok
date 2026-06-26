# Receipt Share Image Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a "daily receipt" share-image feature — the GUI fetches a day's token usage via a new `receipt.daily` control method, renders a polished mall/restaurant-style receipt (its own standalone design language, bundled fonts), and exports it as a PNG (copy to clipboard / save to file / text-summary fallback) from a Share button placed immediately left of the Overview toolbar's Refresh button.

**Architecture:** Backend adds a `receipt.daily` read-plane method (store queries over the existing `daily_usage` / `usage_events` / `usage_buckets_hour` tables → pure assembler → `ReadEnvelopeDto`), wired through the same protocol→control→runtime chain as `overview.summary`. The GUI adds a `features/receipt/` module: a `ReceiptPaper` component (two renders — dialog preview + off-screen 420px export target — driven by one `ReceiptViewModel` and one `receipt.css`), a Radix preview dialog, a `useReceiptExport` hook (`modern-screenshot` `domToBlob` → clipboard `writeImage` / Rust `save_receipt_png` command / text fallback), and a toolbar integration that composes `<ShareReceiptButton/><RefreshButton/>` via the existing per-page toolbar mechanism. No `tauri-plugin-fs`; a single Rust command writes the file.

**Tech Stack:** Rust (rusqlite, serde, ts-rs, tracing, tokio), React 19 + TanStack Query + Radix Dialog + Zustand + lucide-react, Tauri v2 (`tauri-plugin-dialog`, `tauri-plugin-clipboard-manager`, `modern-screenshot`), Vitest + jsdom, JetBrains Mono + Geist (OFL woff2).

## Global Constraints

[From the spec + verified codebase conventions — bind all tasks.]

- **Always read receipt hero metrics + per-model slices from `daily_usage`** (the only day rollup with the full cache-token split). Do NOT mirror `overview_summary`'s fast path (it reads the cache-less UTC bucket tables). (`read_queries.rs:2121` is the template.)
- **`daily_usage` is single-timezone-only.** `date` is always in the *current* reporting TZ; a TZ change re-projects history. `receipt.daily` resolves `date=None` to today via `ReportingTimezone::local_date_for_timestamp_ms(now_ms)`.
- **Compute the reporting-TZ day window directly** with `rtz.civil_date_to_utc_start_ms(date)` + `rtz.civil_date_to_utc_start_ms(rtz.next_civil_date(date)?)` → `[start_ms, end_ms)`. Do NOT use `range::resolve_range` (preset-anchored to today). DST-safe for IANA zones.
- **`session_count` is MANDATORILY** `COUNT(DISTINCT session_id) FROM usage_events WHERE generation_id=? AND timestamp_ms >= start_ms AND timestamp_ms < end_ms`. Never use `usage_by_session_day.date` (UTC-derived) or `last_active_at_ms` — both silently miscount across the UTC/reporting-TZ boundary. The existing `idx_usage_events_time` index makes the day-window scan efficient; **no new migration** is needed.
- **`cost_status` is derived, not stored**, on `daily_usage` (no such column). Reuse `ui_models::cost_status(has_cost, has_no_cost)` (the same helper `overview_summary` uses) at both the aggregate and per-model levels.
- **DTO registration is manual:** `#[derive(TS)]` alone does NOT surface types in `generated.ts`. Every new DTO must be appended to the `type_defs` vec in `crates/busytok-protocol/src/ts.rs:37` in dependency order, then `scripts/generate_protocol_types.sh` run.
- **Adding a `RuntimeControl` method touches 4 places:** the trait, `BusytokSupervisor` impl, `Arc<T>` blanket impl (`dispatch.rs:944`), `TestRuntimeControl` (`dispatch.rs:461`), plus the dispatcher match arm and `method_manifest()`.
- **Tauri commands return `Result<T, String>`** with `.map_err(|e| format!("...: {e}"))`. `save_receipt_png` writes via `tokio::task::spawn_blocking(|| std::fs::write(...))` — the workspace `tokio` dep does NOT enable the `fs` feature.
- **`writeImage` accepts raw `Uint8Array`** (`@tauri-apps/plugin-clipboard-manager@2.3.2`). Do NOT use `Image.fromPngBytes` (does not exist). Capture via `modern-screenshot`'s `domToBlob` (returns a `Blob`), not `domToPng` (returns a data-URL string). The `tauri` crate **already enables `image-png`** in `apps/gui/src-tauri/Cargo.toml:23`, so the raw-PNG-bytes arm of `writeImage` is compiled in — **no Cargo feature change needed** (do not be fooled into adding it).
- **Observability:** Rust uses `tracing` with an `event_code = "receipt.daily_<verb>"` field; the GUI uses `reportFrontendEventSafely({ event_code: "gui.receipt.<verb>", ... })` from `logging/safeReporter.ts` (never the raw reporter on user-action paths).
- **Toolbar memoization:** `useRegisterPageToolbar` re-fires on every element reference change → the registered element MUST be `useMemo`-stabilized (see `useRefreshToolbar.test.tsx:52-58`) or you get a render loop.
- **Design-system guard:** `receipt.css` is intentionally outside `DESIGN-SYSTEM.md` (own palette/radii). It MUST be excluded from `scripts/check-busytok-gui-surfaces.sh` at the **file level** (`--glob '!**/receipt.css'`), never a directory-level glob.
- **Fonts:** bundle JetBrains Mono + Geist woff2 (OFL) under `apps/gui/public/fonts/` (the dir does not exist yet — first font bundling). `@font-face` scoped under `.receipt-paper` so app chrome (system fonts) is untouched.
- **Coverage reality:** Rust receipt store/runtime/protocol/control code IS measured by `scripts/coverage.sh` (workspace gate, default 80, ratchettable via `COVERAGE_GATE`). The Tauri `save_receipt_png` command lives in `busytok-gui` which is EXCLUDED from that gate and tested only on macOS CI. Vitest thresholds are 90% locally but JS coverage is NOT run in CI (vitest hangs — pre-existing). New Rust code targets >90% covered; the workspace gate default stays 80 (bumping risks red CI on legacy modules — out of scope).
- **No `any`/TODO/placeholders/`unimplemented!()`.** `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `pnpm typecheck` must pass before each commit. Per-crate coverage ≥ 90% for touched Rust crates where measured.

---

## File Structure

### Rust

| File | Responsibility |
|------|----------------|
| `crates/busytok-store/src/read_models.rs` | **Modify:** add `ReceiptDailyTotalsRow`, `ReceiptModelSliceRow`, `PeakHourRow` |
| `crates/busytok-store/src/read_queries.rs` | **Modify:** add `read_daily_receipt_totals`, `read_daily_receipt_top_models`, `read_session_count_for_window`, `read_peak_hour_for_window` + inline tests |
| `crates/busytok-protocol/src/dto.rs` | **Modify:** add `ReceiptDailyRequestDto`, `ReceiptDailyDto`, `ReceiptMetricsDto`, `ReceiptPeakHourDto`, `ReceiptModelSliceDto`, `ReceiptBrandDto` |
| `crates/busytok-protocol/src/methods.rs` | **Modify:** add `"receipt.daily"` to `method_manifest()` |
| `crates/busytok-protocol/src/ts.rs` | **Modify:** register the new DTOs in `type_defs` (dependency order) |
| `packages/busytok-protocol-types/src/generated.ts` | **Regenerate:** via `scripts/generate_protocol_types.sh` |
| `crates/busytok-control/src/dispatch.rs` | **Modify:** add `receipt_daily` to the `RuntimeControl` trait, a match arm, the `Arc<T>` blanket forwarder, and `TestRuntimeControl` |
| `crates/busytok-runtime/src/receipt.rs` | **Create:** pure `assemble_receipt_daily(...)` + `ReceiptDailyData` + unit tests |
| `crates/busytok-runtime/src/supervisor.rs` | **Modify:** implement `receipt_daily` (window → 4 store reads → assembler → envelope, with tracing) |
| `crates/busytok-runtime/src/lib.rs` | **Modify:** `pub mod receipt;` |

### Tauri host

| File | Responsibility |
|------|----------------|
| `apps/gui/src-tauri/src/commands.rs` | **Modify:** add `save_receipt_png(path, bytes)` command |
| `apps/gui/src-tauri/src/lib.rs` | **Modify:** register the command in `generate_handler!`; register `tauri_plugin_dialog::init()` |
| `apps/gui/src-tauri/Cargo.toml` | **Modify:** add `tauri-plugin-dialog` |
| `apps/gui/src-tauri/capabilities/default.json` | **Modify:** add `clipboard-manager:allow-write-image`, `dialog:default`, `dialog:allow-save` |

### GUI (TypeScript)

| File | Responsibility |
|------|----------------|
| `apps/gui/package.json` | **Modify:** add `modern-screenshot` + `@tauri-apps/plugin-dialog` deps |
| `apps/gui/src/api/queryKeys.ts` | **Modify:** add `receiptDaily(date)` |
| `apps/gui/src/api/busytokClient.ts` | **Modify:** add `receiptDaily` method + DTO imports |
| `apps/gui/src/api/useBusytokData.ts` | **Modify:** add `useDailyReceipt(date)` hook |
| `apps/gui/src/lib/formatters.ts` | **Modify:** `export formatCostValue` (reuse, one word added) |
| `apps/gui/src/features/receipt/receipt.css` | **Create:** standalone receipt design language + scoped `@font-face` |
| `apps/gui/src/features/receipt/viewModel.ts` | **Create:** `ReceiptViewModel` + `toReceiptViewModel(dto)` (centralizes formatting + top5/OTHERS) |
| `apps/gui/src/features/receipt/ReceiptPaper.tsx` | **Create:** pure visual component |
| `apps/gui/src/features/receipt/ReceiptPaper.test.tsx` | **Create:** fixture render tests |
| `apps/gui/src/features/receipt/fixtures.ts` | **Create:** 6 mock `ReceiptDailyDto` fixtures |
| `apps/gui/src/features/receipt/useReceiptExport.ts` | **Create:** capture + copy + save + summary + logging |
| `apps/gui/src/features/receipt/useReceiptExport.test.ts` | **Create:** mocked flow tests |
| `apps/gui/src/features/receipt/ReceiptPreviewDialog.tsx` | **Create:** Radix dialog: preview + off-screen export root + date control + actions |
| `apps/gui/src/features/receipt/ReceiptPreviewDialog.test.tsx` | **Create:** dialog open/close + action wiring tests |
| `apps/gui/src/features/receipt/ShareReceiptButton.tsx` | **Create:** toolbar icon button |
| `apps/gui/src/components/desktop/useRefreshClickHandler.ts` | **Create:** extracted shared refresh-click handler (reused by `useRefreshToolbar` + the receipt toolbar) |
| `apps/gui/src/components/desktop/useRefreshToolbar.tsx` | **Modify:** use the extracted handler (delete inline duplicate) |
| `apps/gui/src/features/receipt/useReceiptToolbar.tsx` | **Create:** registers `<><ShareReceiptButton/><RefreshButton/></>` for the Overview page |
| `apps/gui/src/pages/OverviewPage.tsx` | **Modify:** call `useReceiptToolbar` instead of `useRefreshToolbar` |

### Scripts

| File | Responsibility |
|------|----------------|
| `scripts/check-busytok-gui-surfaces.sh` | **Modify:** add `--glob '!**/receipt.css'` to the 3 CSS `rg` passes |

---

## Task Decomposition

### Task 1: Store — receipt read queries + row models

**Files:**
- Modify: `crates/busytok-store/src/read_models.rs`
- Modify: `crates/busytok-store/src/read_queries.rs`
- Test: inline `#[cfg(test)]` in `read_queries.rs`

**Interfaces:**
- Produces: `ReceiptDailyTotalsRow`, `ReceiptModelSliceRow`, `PeakHourRow` (read_models); `read_daily_receipt_totals(conn, timezone, date, generation_id) -> Result<ReceiptDailyTotalsRow>`, `read_daily_receipt_top_models(conn, timezone, date, generation_id) -> Result<Vec<ReceiptModelSliceRow>>`, `read_session_count_for_window(conn, generation_id, start_ms, end_ms) -> Result<i64>`, `read_peak_hour_for_window(conn, generation_id, start_ms, end_ms) -> Result<Option<PeakHourRow>>` (read_queries).
- Consumes: `rusqlite::{Connection, params, OptionalExtension}`, `anyhow::{Context, Result}`.

- [ ] **Step 1: Add the three row structs to `read_models.rs`**

Append after `DailyUsageTrendRow` (near `read_models.rs:371`):

```rust
/// Hero token/cost totals for one receipt day (aggregated from `daily_usage`,
/// single date + timezone + generation). `has_cost`/`has_no_cost` drive the
/// derived `cost_status` (the column does not exist on `daily_usage`).
#[derive(Debug, Clone)]
pub struct ReceiptDailyTotalsRow {
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cost_usd: Option<f64>,
    pub has_cost: bool,
    pub has_no_cost: bool,
    pub event_count: i64,
}

/// One model's day slice for the receipt items section.
#[derive(Debug, Clone)]
pub struct ReceiptModelSliceRow {
    pub name: String,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
    pub has_cost: bool,
    pub has_no_cost: bool,
    pub event_count: i64,
}

/// The highest-token UTC hour bucket within the receipt day window.
/// `bucket_start_ms` is UTC-aligned; the caller converts it to a reporting-TZ
/// hour label.
#[derive(Debug, Clone)]
pub struct PeakHourRow {
    pub bucket_start_ms: i64,
    pub tokens: i64,
}
```

- [ ] **Step 2: Write the failing tests in `read_queries.rs`**

Append to the `#[cfg(test)]` block at the bottom of `read_queries.rs`:

```rust
#[test]
fn read_daily_receipt_totals_aggregates_one_date() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn();
    for model in ["claude-sonnet-4-5", "gpt-5.1"] {
        conn.execute(
            "INSERT INTO daily_usage (date, timezone, agent, project_hash, model, \
             input_tokens, output_tokens, total_tokens, cached_input_tokens, \
             cache_creation_tokens, cache_read_tokens, reasoning_tokens, \
             thoughts_tokens, tool_tokens, cost_usd, estimated_cost_usd, event_count, generation_id) \
             VALUES ('2026-06-26', 'Asia/Shanghai', 'claude_code', '', ?1, \
             100, 200, 300, 0, 10, 90, 0, 0, 0, 0.05, NULL, 1, 'gen-r')",
            params![model],
        )
        .unwrap();
    }
    // A row with NULL cost on a different date — must be excluded.
    conn.execute(
        "INSERT INTO daily_usage (date, timezone, agent, project_hash, model, \
         input_tokens, output_tokens, total_tokens, cached_input_tokens, \
         cache_creation_tokens, cache_read_tokens, reasoning_tokens, \
         thoughts_tokens, tool_tokens, cost_usd, estimated_cost_usd, event_count, generation_id) \
         VALUES ('2026-06-27', 'Asia/Shanghai', 'claude_code', '', 'claude-sonnet-4-5', \
         1, 1, 2, 0, 0, 0, 0, 0, 0, NULL, NULL, 1, 'gen-r')",
        [],
    )
    .unwrap();

    let t = read_daily_receipt_totals(conn, "Asia/Shanghai", "2026-06-26", "gen-r").unwrap();
    assert_eq!(t.total_tokens, 600); // 300 + 300
    assert_eq!(t.input_tokens, 200);
    assert_eq!(t.output_tokens, 400);
    assert_eq!(t.cache_read_tokens, 180);
    assert_eq!(t.cache_creation_tokens, 20);
    assert_eq!(t.cost_usd, Some(0.10));
    assert_eq!(t.event_count, 2);
    assert!(t.has_cost);
    assert!(!t.has_no_cost);
}

#[test]
fn read_daily_receipt_totals_empty_day_is_zero() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn();
    let t = read_daily_receipt_totals(conn, "Asia/Shanghai", "2026-06-26", "gen-r").unwrap();
    assert_eq!(t.total_tokens, 0);
    assert_eq!(t.event_count, 0);
    assert!(t.cost_usd.is_none());
    assert!(!t.has_cost);
    assert!(!t.has_no_cost); // no rows → neither flag set
}

#[test]
fn read_daily_receipt_top_models_orders_by_tokens_and_flags_partial_cost() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn();
    // model A: exact cost, 300 tokens. model B: NULL cost (partial), 500 tokens.
    conn.execute(
        "INSERT INTO daily_usage (date, timezone, agent, project_hash, model, \
         input_tokens, output_tokens, total_tokens, cached_input_tokens, \
         cache_creation_tokens, cache_read_tokens, reasoning_tokens, \
         thoughts_tokens, tool_tokens, cost_usd, estimated_cost_usd, event_count, generation_id) \
         VALUES ('2026-06-26', 'UTC', 'claude_code', '', 'model-a', \
         100, 200, 300, 0, 0, 0, 0, 0, 0, 0.05, NULL, 1, 'gen-r')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO daily_usage (date, timezone, agent, project_hash, model, \
         input_tokens, output_tokens, total_tokens, cached_input_tokens, \
         cache_creation_tokens, cache_read_tokens, reasoning_tokens, \
         thoughts_tokens, tool_tokens, cost_usd, estimated_cost_usd, event_count, generation_id) \
         VALUES ('2026-06-26', 'UTC', 'claude_code', '', 'model-b', \
         200, 300, 500, 0, 0, 0, 0, 0, 0, NULL, NULL, 1, 'gen-r')",
        [],
    )
    .unwrap();

    let models = read_daily_receipt_top_models(conn, "UTC", "2026-06-26", "gen-r").unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].name, "model-b"); // 500 > 300
    assert_eq!(models[0].tokens, 500);
    assert!(models[0].has_no_cost);
    assert!(!models[0].has_cost);
    assert!(models[1].has_cost);
    assert!(!models[1].has_no_cost);
}

#[test]
fn read_session_count_uses_ms_window_not_utc_date() {
    // Regression: the +08:00 Jun-26 day window is [2026-06-25T16:00Z,
    // 2026-06-26T16:00Z). sess-a fires at 2026-06-25T17:00Z — its UTC date is
    // Jun 25 but it belongs to the reporting-TZ Jun 26 day, so a UTC-date-based
    // count (usage_by_session_day.date) would miss it. The ms-window count on
    // usage_events must include it. All NOT NULL no-default usage_events columns
    // are populated (schema 0001_baseline.sql:33-77).
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn();
    let start_ms = 1_782_403_200_000i64; // 2026-06-25T16:00:00Z = +08:00 Jun 26 00:00
    let end_ms = start_ms + 86_400_000;

    let insert_event = |conn: &Connection, id: &str, session: &str, ts_ms: i64, dk: &str| {
        conn.execute(
            "INSERT INTO usage_events (id, agent, source_file_id, source_path, source_line, \
             source_offset_start, source_offset_end, session_id, timestamp_ms, model, \
             total_tokens, cost_usd, cost_source, raw_event_hash, is_error, generation_id, \
             dedupe_key, created_at_ms, updated_at_ms) \
             VALUES (?1, 'claude_code', 'f1', '/tmp/t.jsonl', 0, 0, 0, ?2, ?3, 'm', 10, NULL, \
             'unknown', '', 0, 'gen-r', ?4, 0, 0)",
            params![id, session, ts_ms, dk],
        )
        .unwrap();
    };

    // sess-a: 2026-06-25T17:00Z — UTC date Jun 25, reporting-TZ date Jun 26 (boundary).
    insert_event(conn, "e1", "sess-a", start_ms + 3_600_000, "dk1");
    insert_event(conn, "e2", "sess-a", start_ms + 3_660_000, "dk2"); // same session
    // sess-b: 2026-06-26T10:00Z — clearly inside the window.
    insert_event(conn, "e3", "sess-b", start_ms + 64_800_000, "dk3");
    // sess-c: outside the window — must be excluded.
    insert_event(conn, "e4", "sess-c", end_ms + 1, "dk4");

    let count = read_session_count_for_window(conn, "gen-r", start_ms, end_ms).unwrap();
    assert_eq!(count, 2, "sess-a and sess-b are in the window; sess-c is not");
}

#[test]
fn read_peak_hour_for_window_picks_max_bucket() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn();
    conn.execute(
        "INSERT INTO usage_buckets_hour (generation_id, bucket_start_ms, agent, model, \
         total_tokens, cost_status, event_count, created_at_ms, updated_at_ms) \
         VALUES ('gen-r', ?1, 'claude_code', 'm', 100, 'exact', 1, 0, 0)",
        params![1_781_600_400_000i64],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO usage_buckets_hour (generation_id, bucket_start_ms, agent, model, \
         total_tokens, cost_status, event_count, created_at_ms, updated_at_ms) \
         VALUES ('gen-r', ?1, 'claude_code', 'm', 500, 'exact', 1, 0, 0)",
        params![1_781_600_400_000i64 + 3_600_000],
    )
    .unwrap();
    let start_ms = 1_781_600_000_000i64;
    let peak =
        read_peak_hour_for_window(conn, "gen-r", start_ms, start_ms + 86_400_000).unwrap();
    let peak = peak.expect("a peak bucket exists");
    assert_eq!(peak.tokens, 500);
    assert_eq!(peak.bucket_start_ms, 1_781_600_400_000i64 + 3_600_000);
}

#[test]
fn read_peak_hour_for_window_empty_is_none() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn();
    let peak =
        read_peak_hour_for_window(conn, "gen-r", 0, 86_400_000).unwrap();
    assert!(peak.is_none());
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p busytok-store read_daily_receipt read_session_count read_peak_hour`
Expected: FAIL — functions not defined.

- [ ] **Step 4: Implement the four query functions in `read_queries.rs`**

Append near the other `read_overview_*_from_daily_usage` functions (after `read_overview_trend_from_daily_usage`):

```rust
/// Hero token/cost totals for one receipt day from `daily_usage`.
/// (The row structs resolve via the file's existing `use crate::read_models::*;`
/// glob import — do not re-import them.)
pub fn read_daily_receipt_totals(
    conn: &Connection,
    timezone: &str,
    date: &str,
    generation_id: &str,
) -> Result<ReceiptDailyTotalsRow> {
    conn.query_row(
        "SELECT
            COALESCE(SUM(total_tokens), 0),
            COALESCE(SUM(input_tokens), 0),
            COALESCE(SUM(output_tokens), 0),
            COALESCE(SUM(cache_read_tokens), 0),
            COALESCE(SUM(cache_creation_tokens), 0),
            SUM(cost_usd),
            COALESCE(SUM(event_count), 0),
            COALESCE(SUM(CASE WHEN cost_usd IS NOT NULL THEN event_count ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN cost_usd IS NULL THEN event_count ELSE 0 END), 0)
         FROM daily_usage
         WHERE timezone = ?1 AND date = ?2 AND generation_id = ?3",
        params![timezone, date, generation_id],
        |row| {
            let with_cost: i64 = row.get(7)?;
            let without_cost: i64 = row.get(8)?;
            Ok(ReceiptDailyTotalsRow {
                total_tokens: row.get(0)?,
                input_tokens: row.get(1)?,
                output_tokens: row.get(2)?,
                cache_read_tokens: row.get(3)?,
                cache_creation_tokens: row.get(4)?,
                cost_usd: row.get(5)?,
                event_count: row.get(6)?,
                has_cost: with_cost > 0,
                has_no_cost: without_cost > 0,
            })
        },
    )
    .context("failed to read daily receipt totals")
}

/// Per-model day slices for the receipt items section, ranked by tokens desc.
pub fn read_daily_receipt_top_models(
    conn: &Connection,
    timezone: &str,
    date: &str,
    generation_id: &str,
) -> Result<Vec<ReceiptModelSliceRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT
                model,
                COALESCE(SUM(total_tokens), 0),
                SUM(cost_usd),
                COALESCE(SUM(event_count), 0),
                COALESCE(SUM(CASE WHEN cost_usd IS NOT NULL THEN event_count ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN cost_usd IS NULL THEN event_count ELSE 0 END), 0)
             FROM daily_usage
             WHERE timezone = ?1 AND date = ?2 AND generation_id = ?3
             GROUP BY model
             ORDER BY COALESCE(SUM(total_tokens), 0) DESC, model ASC",
        )
        .context("failed to prepare read_daily_receipt_top_models")?;
    let rows = stmt.query_map(params![timezone, date, generation_id], |row| {
        let with_cost: i64 = row.get(4)?;
        let without_cost: i64 = row.get(5)?;
        Ok(ReceiptModelSliceRow {
            name: row.get(0)?,
            tokens: row.get(1)?,
            cost_usd: row.get(2)?,
            event_count: row.get(3)?,
            has_cost: with_cost > 0,
            has_no_cost: without_cost > 0,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
}

/// Distinct session count within a reporting-TZ `[start_ms, end_ms)` window.
///
/// Deliberately queries `usage_events` by `timestamp_ms` — NOT
/// `usage_by_session_day.date` (UTC-derived) nor `last_active_at_ms`, both of
/// which miscount sessions that straddle the UTC/reporting-TZ day boundary.
/// `idx_usage_events_time` makes the day-window scan efficient.
pub fn read_session_count_for_window(
    conn: &Connection,
    generation_id: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(DISTINCT session_id)
         FROM usage_events
         WHERE generation_id = ?1
           AND timestamp_ms >= ?2 AND timestamp_ms < ?3
           AND session_id IS NOT NULL AND session_id <> ''",
        params![generation_id, start_ms, end_ms],
        |row| row.get(0),
    )
    .context("failed to read session count for receipt window")
}

/// The highest-token UTC hour bucket in `[start_ms, end_ms)`, or `None` if the
/// day has no buckets. Caller converts `bucket_start_ms` to a reporting-TZ hour.
pub fn read_peak_hour_for_window(
    conn: &Connection,
    generation_id: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<Option<PeakHourRow>> {
    conn.query_row(
        "SELECT bucket_start_ms, COALESCE(SUM(total_tokens), 0) AS t
         FROM usage_buckets_hour
         WHERE generation_id = ?1
           AND bucket_start_ms >= ?2 AND bucket_start_ms < ?3
         GROUP BY bucket_start_ms
         ORDER BY t DESC, bucket_start_ms ASC
         LIMIT 1",
        params![generation_id, start_ms, end_ms],
        |row| {
            Ok(PeakHourRow {
                bucket_start_ms: row.get(0)?,
                tokens: row.get(1)?,
            })
        },
    )
    .optional()
    .context("failed to read peak hour for receipt window")
}
```

(If `OptionalExtension` is not already in the file's `use rusqlite::...` line — it is, per `read_queries.rs:4` — do not re-import. The `use crate::read_models::{...}` line should be merged into the existing `use crate::read_models::*;` at the top; if `*` is used, drop the explicit import.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p busytok-store read_daily_receipt read_session_count read_peak_hour`
Expected: PASS (6 tests).

- [ ] **Step 6: Lint, format, commit**

```bash
cargo fmt --all
cargo clippy -p busytok-store --all-targets -- -D warnings
git add crates/busytok-store/src/read_models.rs crates/busytok-store/src/read_queries.rs
git commit -m "feat(store): add daily receipt read queries (totals, top models, session count, peak hour)"
```

---

### Task 2: Protocol — `receipt.daily` DTOs + manifest + TS registration

**Files:**
- Modify: `crates/busytok-protocol/src/dto.rs`
- Modify: `crates/busytok-protocol/src/methods.rs`
- Modify: `crates/busytok-protocol/src/ts.rs`
- Regenerate: `packages/busytok-protocol-types/src/generated.ts`

**Interfaces:**
- Produces: `ReceiptDailyRequestDto { date: Option<String> }`, `ReceiptDailyDto`, `ReceiptMetricsDto`, `ReceiptPeakHourDto`, `ReceiptModelSliceDto`, `ReceiptBrandDto` (all `#[derive(Debug, Clone, Serialize, Deserialize, TS)]`). Reuses `CostStatusDto`.
- Consumes: existing `CostStatusDto`, `ReadEnvelopeDto` (wrapper applied in the runtime, not here).

- [ ] **Step 1: Write a failing serialization test**

In `crates/busytok-protocol/src/dto.rs`, add to its `#[cfg(test)]` block (or create one mirroring neighbors):

```rust
#[test]
fn receipt_daily_dto_round_trips() {
    let dto = ReceiptDailyDto {
        date: "2026-06-26".to_string(),
        date_label: "FRI · JUN 26, 2026".to_string(),
        timezone: "Asia/Shanghai".to_string(),
        metrics: ReceiptMetricsDto {
            total_tokens: 100,
            input_tokens: 40,
            output_tokens: 60,
            cache_read_tokens: 30,
            cache_creation_tokens: 5,
            cache_hit_rate: Some(0.42857),
            cost_usd: Some(1.23),
            cost_status: CostStatusDto::Exact,
            event_count: 7,
            session_count: 2,
            peak_hour: Some(ReceiptPeakHourDto {
                label: "14:00".to_string(),
                tokens: 80,
            }),
        },
        top_models: vec![ReceiptModelSliceDto {
            name: "claude-sonnet-4-5".to_string(),
            tokens: 100,
            cost_usd: Some(1.23),
            cost_status: CostStatusDto::Exact,
        }],
        brand: ReceiptBrandDto {
            name: "BUSYTOK".to_string(),
            tagline: "AI CODING · TOKEN RECEIPT".to_string(),
            github: "github.com/BalianWang/busytok".to_string(),
            generated_at_ms: 1_781_600_000_000,
        },
    };
    let json = serde_json::to_string(&dto).unwrap();
    let back: ReceiptDailyDto = serde_json::from_str(&json).unwrap();
    assert_eq!(back.date, dto.date);
    assert_eq!(back.metrics.cost_status, CostStatusDto::Exact);
    assert_eq!(back.top_models.len(), 1);
    assert_eq!(back.metrics.peak_hour.unwrap().label, "14:00");
}

#[test]
fn receipt_request_date_defaults_none() {
    let req = ReceiptDailyRequestDto { date: None };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("\"date\":null"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p busytok-protocol receipt_daily`
Expected: FAIL — types not defined.

- [ ] **Step 3: Add the DTOs to `dto.rs`**

Append (reusing the file's existing `use serde::{Deserialize, Serialize}; use ts_rs::TS;`):

```rust
// ── Receipt (daily share image) ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReceiptDailyRequestDto {
    /// `YYYY-MM-DD` in the current reporting timezone. `None` = today
    /// (server-resolved). See `receipt.daily` spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReceiptDailyDto {
    pub date: String,
    /// Server-produced label, e.g. "FRI · JUN 26, 2026". Format semantics
    /// intentionally match the GUI's `src/lib/formatters.ts`; produced
    /// server-side so the future Rust render path can share the ViewModel.
    pub date_label: String,
    pub timezone: String,
    pub metrics: ReceiptMetricsDto,
    pub top_models: Vec<ReceiptModelSliceDto>,
    pub brand: ReceiptBrandDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReceiptMetricsDto {
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    /// `cache_read_tokens / (input_tokens + cache_read_tokens)`, else `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_hit_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub cost_status: CostStatusDto,
    pub event_count: i64,
    pub session_count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_hour: Option<ReceiptPeakHourDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReceiptPeakHourDto {
    /// Reporting-TZ wall-clock hour, e.g. "14:00".
    pub label: String,
    pub tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReceiptModelSliceDto {
    pub name: String,
    pub tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub cost_status: CostStatusDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReceiptBrandDto {
    pub name: String,
    pub tagline: String,
    pub github: String,
    pub generated_at_ms: i64,
}
```

- [ ] **Step 4: Register the method in `method_manifest()`**

In `crates/busytok-protocol/src/methods.rs`, add inside the `vec![` (e.g. after the `// Overview` block):

```rust
        // Receipt
        "receipt.daily".to_string(),
```

- [ ] **Step 5: Register DTOs in `ts.rs` `type_defs`**

In `crates/busytok-protocol/src/ts.rs`, add to the `type_defs` vec in **dependency order** (leaves first — `CostStatusDto` is already present earlier in the vec; `ReceiptPeakHourDto`, `ReceiptModelSliceDto`, `ReceiptBrandDto`, `ReceiptMetricsDto`, then `ReceiptDailyRequestDto`, then `ReceiptDailyDto`):

```rust
        // Receipt (daily share image)
        dto::ReceiptPeakHourDto::decl(),
        dto::ReceiptModelSliceDto::decl(),
        dto::ReceiptBrandDto::decl(),
        dto::ReceiptMetricsDto::decl(),
        dto::ReceiptDailyRequestDto::decl(),
        dto::ReceiptDailyDto::decl(),
```

- [ ] **Step 6: Run tests + regenerate TS types**

```bash
cargo test -p busytok-protocol receipt_daily
bash scripts/generate_protocol_types.sh
```
Expected: test PASS; `packages/busytok-protocol-types/src/generated.ts` now contains `export type ReceiptDailyDto = ...` etc.

- [ ] **Step 7: Verify generated types + commit**

```bash
grep -n "ReceiptDailyDto" packages/busytok-protocol-types/src/generated.ts
cargo fmt --all
cargo clippy -p busytok-protocol --all-targets -- -D warnings
git add crates/busytok-protocol/src/dto.rs crates/busytok-protocol/src/methods.rs crates/busytok-protocol/src/ts.rs packages/busytok-protocol-types/src/generated.ts
git commit -m "feat(protocol): add receipt.daily DTOs + method manifest + TS registration"
```

---

### Task 3: Control + Runtime — dispatch wiring, pure assembler, supervisor handler

**Files:**
- Modify: `crates/busytok-control/src/dispatch.rs`
- Create: `crates/busytok-runtime/src/receipt.rs`
- Modify: `crates/busytok-runtime/src/lib.rs`
- Modify: `crates/busytok-runtime/src/supervisor.rs`

**Interfaces:**
- Produces: `RuntimeControl::receipt_daily` (trait method); `busytok_runtime::receipt::assemble_receipt_daily(data, rtz, date, now_ms) -> Result<ReceiptDailyDto>` (pure) + `ReceiptDailyData { totals, models, session_count, peak_hour }`.
- Consumes: the four store fns (Task 1), `ui_models::cost_status`, `range::parse_timezone`, `ReportingTimezone::{local_date_for_timestamp_ms, civil_date_to_utc_start_ms, next_civil_date, canonical_name}`, `busytok_domain::now_ms`, `build_read_envelope`, `run_read_with_mode`.

- [ ] **Step 1: Add `receipt_daily` to the `RuntimeControl` trait**

In `crates/busytok-control/src/dispatch.rs`, add to the trait (near `overview_summary`):

```rust
    async fn receipt_daily(
        &self,
        req: ReceiptDailyRequestDto,
    ) -> Result<ReadEnvelopeDto<ReceiptDailyDto>>;
```

Add the import to the `use busytok_protocol::dto::{...}` block: `ReceiptDailyDto, ReceiptDailyRequestDto`.

- [ ] **Step 2: Add the dispatcher match arm**

In the `match request.method.as_str()` block (near the `"overview.summary"` arm):

```rust
            "receipt.daily" => {
                let req: ReceiptDailyRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for receipt.daily: {e}"))?;
                let dto = self.runtime.receipt_daily(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
```

- [ ] **Step 3: Add the `Arc<T>` blanket forwarder**

In the `impl<T: RuntimeControl> RuntimeControl for Arc<T>` block:

```rust
    async fn receipt_daily(
        &self,
        req: ReceiptDailyRequestDto,
    ) -> Result<ReadEnvelopeDto<ReceiptDailyDto>> {
        (**self).receipt_daily(req).await
    }
```

- [ ] **Step 4: Stub `TestRuntimeControl`**

In `impl RuntimeControl for TestRuntimeControl`:

```rust
    async fn receipt_daily(
        &self,
        req: ReceiptDailyRequestDto,
    ) -> Result<ReadEnvelopeDto<ReceiptDailyDto>> {
        Ok(ReadEnvelopeDto {
            data: ReceiptDailyDto {
                date: req.date.unwrap_or_else(|| "2026-06-26".to_string()),
                date_label: "FRI · JUN 26, 2026".to_string(),
                timezone: "UTC".to_string(),
                metrics: ReceiptMetricsDto {
                    total_tokens: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_hit_rate: None,
                    cost_usd: None,
                    cost_status: CostStatusDto::Unavailable,
                    event_count: 0,
                    session_count: 0,
                    peak_hour: None,
                },
                top_models: vec![],
                brand: ReceiptBrandDto {
                    name: "BUSYTOK".to_string(),
                    tagline: "AI CODING · TOKEN RECEIPT".to_string(),
                    github: "github.com/BalianWang/busytok".to_string(),
                    generated_at_ms: 0,
                },
            },
            generated_at_ms: 0,
            generation_id: None,
            readiness: ReadinessStateDto::Starting,
            is_exact: false,
            is_stale: true,
            watermark_ms: None,
            progress: None,
            degraded_reason: None,
        })
    }
```

Add the imports (`ReceiptBrandDto, ReceiptDailyDto, ReceiptDailyRequestDto, ReceiptMetricsDto`) to `TestRuntimeControl`'s scope if not already covered by a glob import.

- [ ] **Step 5: Write the failing assembler tests**

Create `crates/busytok-runtime/src/receipt.rs`:

```rust
//! Pure assembly of the daily receipt DTO from store rows. Kept separate from
//! the supervisor so the shaping logic (cost-status mapping, cache-hit rate,
//! peak-hour TZ labelling, date label) is unit-testable without a DB.

use anyhow::Result;
use busytok_domain::ReportingTimezone;
use busytok_protocol::dto::{
    CostStatusDto, ReceiptBrandDto, ReceiptDailyDto, ReceiptMetricsDto, ReceiptModelSliceDto,
    ReceiptPeakHourDto,
};
use busytok_store::read_models::{
    PeakHourRow, ReceiptDailyTotalsRow, ReceiptModelSliceRow,
};

use crate::ui_models;

/// The four store reads bundled for one `receipt.daily` call.
pub struct ReceiptDailyData {
    pub totals: ReceiptDailyTotalsRow,
    pub models: Vec<ReceiptModelSliceRow>,
    pub session_count: i64,
    pub peak_hour: Option<PeakHourRow>,
}

const BRAND_NAME: &str = "BUSYTOK";
const BRAND_TAGLINE: &str = "AI CODING · TOKEN RECEIPT";
const BRAND_GITHUB: &str = "github.com/BalianWang/busytok";

/// Map store rows + the reporting timezone into the wire DTO. Pure: no I/O.
pub fn assemble_receipt_daily(
    data: ReceiptDailyData,
    rtz: &ReportingTimezone,
    date: &str,
    now_ms: i64,
) -> Result<ReceiptDailyDto> {
    let ReceiptDailyData {
        totals,
        models,
        session_count,
        peak_hour,
    } = data;

    let cost_status = ui_models::cost_status(totals.has_cost, totals.has_no_cost);
    let cache_hit_rate = cache_hit_rate(totals.cache_read_tokens, totals.input_tokens);
    let peak_hour_dto = peak_hour_label(peak_hour, rtz)?;

    let top_models = models
        .into_iter()
        .map(|m| ReceiptModelSliceDto {
            name: m.name,
            tokens: m.tokens,
            cost_usd: m.cost_usd,
            cost_status: ui_models::cost_status(m.has_cost, m.has_no_cost),
        })
        .collect();

    Ok(ReceiptDailyDto {
        date: date.to_string(),
        date_label: format_date_label(date)?,
        timezone: rtz.canonical_name().to_string(),
        metrics: ReceiptMetricsDto {
            total_tokens: totals.total_tokens,
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
            cache_read_tokens: totals.cache_read_tokens,
            cache_creation_tokens: totals.cache_creation_tokens,
            cache_hit_rate,
            cost_usd: totals.cost_usd,
            cost_status,
            event_count: totals.event_count,
            session_count,
            peak_hour: peak_hour_dto,
        },
        top_models,
        brand: ReceiptBrandDto {
            name: BRAND_NAME.to_string(),
            tagline: BRAND_TAGLINE.to_string(),
            github: BRAND_GITHUB.to_string(),
            generated_at_ms: now_ms,
        },
    })
}

fn cache_hit_rate(cache_read: i64, input: i64) -> Option<f64> {
    let denom = input + cache_read;
    if denom <= 0 {
        return None;
    }
    Some(cache_read as f64 / denom as f64)
}

/// Convert the peak UTC hour bucket to a reporting-TZ wall-clock label.
/// NOTE: hour = (bucket - local_midnight) / 3_600_000. Exact for whole-hour
/// offsets and for IANA zones except the single DST-transition hour per year
/// (where it may be ±1); acceptable for a secondary receipt metric.
fn peak_hour_label(peak: Option<PeakHourRow>, rtz: &ReportingTimezone) -> Result<Option<ReceiptPeakHourDto>> {
    let Some(p) = peak else {
        return Ok(None);
    };
    if p.tokens <= 0 {
        return Ok(None);
    }
    let local_date = rtz.local_date_for_timestamp_ms(p.bucket_start_ms)?;
    let local_midnight_ms = rtz.civil_date_to_utc_start_ms(&local_date)?;
    let local_hour = ((p.bucket_start_ms - local_midnight_ms) / 3_600_000).rem_euclid(24);
    Ok(Some(ReceiptPeakHourDto {
        label: format!("{local_hour:02}:00"),
        tokens: p.tokens,
    }))
}

/// "YYYY-MM-DD" → "FRI · JUN 26, 2026".
fn format_date_label(date: &str) -> Result<String> {
    let parts: Vec<&str> = date.split('-').collect();
    anyhow::ensure!(parts.len() == 3, "invalid date: {date}");
    let year: i32 = parts[0].parse()?;
    let month: u8 = parts[1].parse()?;
    let day: u8 = parts[2].parse()?;
    use time::{Date, Month};
    let d = Date::from_calendar_date(year, Month::try_from(month).map_err(|e| anyhow::anyhow!("{e}"))?, day)?;
    let wd = match d.weekday() {
        time::Weekday::Monday => "MON",
        time::Weekday::Tuesday => "TUE",
        time::Weekday::Wednesday => "WED",
        time::Weekday::Thursday => "THU",
        time::Weekday::Friday => "FRI",
        time::Weekday::Saturday => "SAT",
        time::Weekday::Sunday => "SUN",
    };
    let mon = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"]
        [(month as usize) - 1];
    Ok(format!("{wd} · {mon} {day}, {year}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn totals(has_cost: bool, has_no_cost: bool) -> ReceiptDailyTotalsRow {
        ReceiptDailyTotalsRow {
            total_tokens: 1000,
            input_tokens: 600,
            output_tokens: 400,
            cache_read_tokens: 300,
            cache_creation_tokens: 50,
            cost_usd: if has_cost { Some(2.5) } else { None },
            has_cost,
            has_no_cost,
            event_count: 9,
        }
    }

    fn rtz() -> ReportingTimezone {
        ReportingTimezone::parse("Asia/Shanghai").unwrap()
    }

    #[test]
    fn maps_aggregate_cost_status_and_cache_hit_rate() {
        let dto = assemble_receipt_daily(
            ReceiptDailyData {
                totals: totals(true, false),
                models: vec![],
                session_count: 3,
                peak_hour: None,
            },
            &rtz(),
            "2026-06-26",
            1_000,
        )
        .unwrap();
        assert_eq!(dto.metrics.cost_status, CostStatusDto::Exact);
        assert!((dto.metrics.cache_hit_rate.unwrap() - (300.0 / 900.0)).abs() < 1e-9);
        assert_eq!(dto.metrics.session_count, 3);
        assert!(dto.metrics.peak_hour.is_none());
        assert_eq!(dto.date_label, "FRI · JUN 26, 2026");
        assert_eq!(dto.brand.name, "BUSYTOK");
    }

    #[test]
    fn partial_cost_when_any_row_lacks_cost() {
        let dto = assemble_receipt_daily(
            ReceiptDailyData {
                totals: totals(true, true),
                models: vec![],
                session_count: 0,
                peak_hour: None,
            },
            &rtz(),
            "2026-06-26",
            0,
        )
        .unwrap();
        assert_eq!(dto.metrics.cost_status, CostStatusDto::Partial);
    }

    #[test]
    fn empty_day_is_unavailable_no_cache_rate() {
        let dto = assemble_receipt_daily(
            ReceiptDailyData {
                totals: ReceiptDailyTotalsRow {
                    total_tokens: 0, input_tokens: 0, output_tokens: 0,
                    cache_read_tokens: 0, cache_creation_tokens: 0,
                    cost_usd: None, has_cost: false, has_no_cost: false, event_count: 0,
                },
                models: vec![],
                session_count: 0,
                peak_hour: None,
            },
            &rtz(),
            "2026-06-26",
            0,
        )
        .unwrap();
        assert_eq!(dto.metrics.cost_status, CostStatusDto::Unavailable);
        assert!(dto.metrics.cache_hit_rate.is_none());
    }

    #[test]
    fn per_model_cost_status_independent_of_aggregate() {
        let data = ReceiptDailyData {
            totals: totals(true, false),
            models: vec![ReceiptModelSliceRow {
                name: "model-b".into(), tokens: 500, cost_usd: None,
                has_cost: false, has_no_cost: true, event_count: 1,
            }],
            session_count: 1,
            peak_hour: None,
        };
        let dto = assemble_receipt_daily(data, &rtz(), "2026-06-26", 0).unwrap();
        assert_eq!(dto.metrics.cost_status, CostStatusDto::Exact); // aggregate has full cost
        assert_eq!(dto.top_models[0].cost_status, CostStatusDto::Unavailable); // that row has none
    }

    #[test]
    fn cache_hit_rate_none_when_denominator_zero() {
        assert!(cache_hit_rate(0, 0).is_none());
        assert!(cache_hit_rate(10, -10).is_none()); // guarded against negative
    }
}
```

- [ ] **Step 6: Register the module + run assembler tests**

In `crates/busytok-runtime/src/lib.rs`, add `pub mod receipt;` (next to the other module declarations).

Run: `cargo test -p busytok-runtime receipt::`
Expected: PASS (5 tests). (If `ui_models::cost_status` is not at `crate::ui_models::cost_status`, locate it — it is used in `supervisor.rs` as `ui_models::cost_status`; adjust the `use` accordingly.)

- [ ] **Step 7: Implement the supervisor handler**

In `crates/busytok-runtime/src/supervisor.rs`, add the import `use busytok_protocol::dto::{ReceiptDailyRequestDto, ReceiptDailyDto};` (merge into the existing glob/use) and implement the trait method on `BusytokSupervisor`:

```rust
    async fn receipt_daily(
        &self,
        req: ReceiptDailyRequestDto,
    ) -> Result<ReadEnvelopeDto<ReceiptDailyDto>> {
        let now_ms = busytok_domain::now_ms();
        let (timezone, _week_starts_on) = self.timezone_and_weekday();
        let rtz = range::parse_timezone(&timezone)
            .unwrap_or_else(|_| ReportingTimezone::utc());
        let date = match req.date.clone() {
            Some(d) => d,
            None => rtz
                .local_date_for_timestamp_ms(now_ms)
                .unwrap_or_else(|_| "1970-01-01".to_string()),
        };
        let start_ms = rtz.civil_date_to_utc_start_ms(&date)?;
        let end_ms = rtz.civil_date_to_utc_start_ms(&rtz.next_civil_date(&date)?)?;
        let generation_id = self.active_generation_id_from_snapshot().await?;

        let tz_name = rtz.canonical_name().to_string();
        let date_for_closure = date.clone();
        let gen_for_closure = generation_id.clone();
        let data = self
            .run_read_with_mode("receipt.daily", "receipt_daily", true, move |conn| {
                let totals = busytok_store::read_queries::read_daily_receipt_totals(
                    conn,
                    &tz_name,
                    &date_for_closure,
                    &gen_for_closure,
                )?;
                let models = busytok_store::read_queries::read_daily_receipt_top_models(
                    conn,
                    &tz_name,
                    &date_for_closure,
                    &gen_for_closure,
                )?;
                let session_count = busytok_store::read_queries::read_session_count_for_window(
                    conn,
                    &gen_for_closure,
                    start_ms,
                    end_ms,
                )?;
                let peak_hour = busytok_store::read_queries::read_peak_hour_for_window(
                    conn,
                    &gen_for_closure,
                    start_ms,
                    end_ms,
                )?;
                Ok(crate::receipt::ReceiptDailyData {
                    totals,
                    models,
                    session_count,
                    peak_hour,
                })
            })
            .await?;

        let dto = crate::receipt::assemble_receipt_daily(data, &rtz, &date, now_ms)?;
        tracing::info!(
            event_code = "receipt.daily_served",
            date = %date,
            model_count = dto.top_models.len(),
            total_tokens = dto.metrics.total_tokens,
            "served daily receipt"
        );
        self.build_read_envelope(dto, now_ms)
    }
```

Ensure `ReportingTimezone` is imported (it is, per the `overview_summary` handler using it). `run_read_with_mode` + `build_read_envelope` + `active_generation_id_from_snapshot` + `timezone_and_weekday` are all existing `BusytokSupervisor` methods.

- [ ] **Step 8: Build + test the workspace (non-GUI)**

```bash
cargo build --workspace --exclude busytok-gui
cargo test -p busytok-control -p busytok-runtime -p busytok-protocol
cargo clippy --workspace --exclude busytok-gui --all-targets -- -D warnings
```
Expected: PASS (compiles; the new trait method is satisfied on `BusytokSupervisor`, `Arc<BusytokSupervisor>`, and `TestRuntimeControl`; assembler + protocol tests pass).

- [ ] **Step 9: Commit**

```bash
cargo fmt --all
git add crates/busytok-control/src/dispatch.rs crates/busytok-runtime/src/receipt.rs crates/busytok-runtime/src/lib.rs crates/busytok-runtime/src/supervisor.rs
git commit -m "feat(runtime): wire receipt.daily through control dispatch + pure assembler + supervisor handler"
```

---

### Task 4: GUI API — client method, query key, hook

**Files:**
- Modify: `apps/gui/package.json` (add deps early so later tasks resolve)
- Modify: `apps/gui/src/api/queryKeys.ts`
- Modify: `apps/gui/src/api/busytokClient.ts`
- Modify: `apps/gui/src/api/useBusytokData.ts`

**Interfaces:**
- Produces: `queryKeys.receiptDaily(date)`, `client.receiptDaily({ date })`, `useDailyReceipt(date)`.
- Consumes: generated `ReceiptDailyRequestDto`/`ReceiptDailyDto`, `envelopeQueryOptions`, `useBusytokClient`.

- [ ] **Step 1: Add the new npm dependencies**

In `apps/gui/package.json`, add to `dependencies`:

```json
    "modern-screenshot": "^4.7.0",
    "@tauri-apps/plugin-dialog": "^2.4.0",
```

Run: `pnpm install`
Expected: lockfile updates; both packages resolve. (Also add `@tauri-apps/plugin-dialog` here so the JS `save()` import resolves — the Rust crate lands in Task 5.)

- [ ] **Step 2: Add the query key**

In `apps/gui/src/api/queryKeys.ts`, add to the `queryKeys` object:

```ts
  receiptDaily: (date: string) => ["receipt", "daily", date] as const,
```

- [ ] **Step 3: Add the client method**

In `apps/gui/src/api/busytokClient.ts`:

Add to the type-only import from `@busytok/protocol-types`:
```ts
  ReceiptDailyDto,
  ReceiptDailyRequestDto,
```
Add next to `overviewSummary`:
```ts
    receiptDaily: (request: ReceiptDailyRequestDto) =>
      call<ReadEnvelopeDto<ReceiptDailyDto>>("receipt.daily", { ...request }),
```

- [ ] **Step 4: Add the data hook**

In `apps/gui/src/api/useBusytokData.ts`, import the types and add (mirroring `useOverviewSummary`):

```ts
export function useDailyReceipt(date: string) {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<ReceiptDailyDto>>(
    envelopeQueryOptions({
      queryKey: queryKeys.receiptDaily(date),
      queryFn: () => client.receiptDaily({ date }),
    }),
  );
}
```
(Add `ReceiptDailyDto` to the existing `@busytok/protocol-types` import block in this file.)

- [ ] **Step 5: Typecheck + commit**

```bash
pnpm typecheck
git add apps/gui/package.json apps/gui/src/api/queryKeys.ts apps/gui/src/api/busytokClient.ts apps/gui/src/api/useBusytokData.ts pnpm-lock.yaml
git commit -m "feat(gui): add receipt.daily client method, query key, useDailyReceipt hook"
```

---

### Task 5: Tauri host — `save_receipt_png` command + dialog plugin + capabilities

**Files:**
- Modify: `apps/gui/src-tauri/Cargo.toml`
- Modify: `apps/gui/src-tauri/src/commands.rs`
- Modify: `apps/gui/src-tauri/src/lib.rs`
- Modify: `apps/gui/src-tauri/capabilities/default.json`
- Test: inline `#[cfg(test)]` in `commands.rs`

**Interfaces:**
- Produces: `save_receipt_png(path: String, bytes: Vec<u8>) -> Result<(), String>`; registered dialog plugin; `clipboard-manager:allow-write-image` + `dialog:default` + `dialog:allow-save` capabilities.
- Consumes: `tokio::fs`, `tracing`.

- [ ] **Step 1: Write the failing test in `commands.rs`**

```rust
#[cfg(test)]
mod receipt_save_tests {
    use super::save_receipt_png;

    #[tokio::test]
    async fn save_receipt_png_writes_bytes_to_path() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("busytok-receipt-test-{}.png", uuid::Uuid::new_v4()));
        let bytes = vec![1u8, 2, 3, 4];
        save_receipt_png(path.to_string_lossy().to_string(), bytes.clone())
            .await
            .expect("write succeeds");
        let on_disk = std::fs::read(&path).expect("file exists");
        assert_eq!(on_disk, bytes);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn save_receipt_png_errors_on_invalid_path() {
        let err = save_receipt_png("/no/such/dir/x.png".to_string(), vec![0u8]).await;
        assert!(err.is_err());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p busytok-gui receipt_save_tests`
Expected: FAIL — `save_receipt_png` not found.

- [ ] **Step 3: Implement the command**

In `apps/gui/src-tauri/src/commands.rs`, add (no extra `State` needed; `tokio` + `tracing` are already deps):

```rust
/// Write the receipt PNG to a user-chosen path. The path comes from the
/// `tauri-plugin-dialog` `save()` call on the JS side — that plugin only returns
/// a path; this command performs the actual write (with tracing + typed errors).
/// Uses `spawn_blocking` + `std::fs` because the workspace `tokio` dep does not
/// enable the `fs` feature (see root Cargo.toml).
#[tauri::command]
pub async fn save_receipt_png(path: String, bytes: Vec<u8>) -> Result<(), String> {
    tracing::info!(
        event_code = "tauri.save_receipt_png",
        byte_count = bytes.len(),
        "writing receipt PNG to user-chosen path"
    );
    tokio::task::spawn_blocking(move || std::fs::write(&path, &bytes))
        .await
        .map_err(|e| format!("receipt save task join failed: {e}"))?
        .map_err(|e| format!("failed to write receipt PNG: {e}"))
}
```

- [ ] **Step 4: Run the test**

```bash
cargo test -p busytok-gui receipt_save_tests
```
Expected: PASS (2 tests).

- [ ] **Step 5: Register the command + dialog plugin**

In `apps/gui/src-tauri/Cargo.toml` `[dependencies]`, add (matching the sibling `=2.x` pin style):
```toml
tauri-plugin-dialog = "=2.4.0"
```

In `apps/gui/src-tauri/src/lib.rs`:
- In `generate_handler!`, add after `commands::desktop_background_service_repair,`:
  ```rust
            commands::save_receipt_png,
  ```
- In the plugin chain, add after `.plugin(tauri_plugin_process::init())`:
  ```rust
        .plugin(tauri_plugin_dialog::init())
  ```

In `apps/gui/src-tauri/capabilities/default.json`, add to the `permissions` array (after `"clipboard-manager:allow-write-text",`):
```json
    "clipboard-manager:allow-write-image",
    "dialog:default",
    "dialog:allow-save",
```

- [ ] **Step 6: Build the GUI crate + commit**

```bash
cargo build -p busytok-gui   # regenerates gen/schemas/acl-manifests.json → 'dialog' key becomes valid
cargo clippy -p busytok-gui --all-targets -- -D warnings
cargo fmt --all
# Confirm the dialog ACL materialized (capabilities only validate once the plugin is built):
grep -q '"dialog"' apps/gui/src-tauri/gen/schemas/acl-manifests.json && echo "dialog ACL ok"
git add apps/gui/src-tauri/Cargo.toml apps/gui/src-tauri/src/commands.rs apps/gui/src-tauri/src/lib.rs apps/gui/src-tauri/capabilities/default.json
git commit -m "feat(gui): save_receipt_png Tauri command + dialog plugin + clipboard-image capability"
```

If `cargo build` reports a `tauri-plugin` core-version conflict from mixing the `2.3.x` cluster with `tauri-plugin-dialog = "=2.4.0"`, pin dialog to the matching `2.3.x` (e.g. `=2.3.0`) instead.

---

### Task 6: Receipt visual design — fonts + standalone `receipt.css` + guard exclusion

**Files:**
- Create: `apps/gui/public/fonts/` (JetBrains Mono + Geist woff2)
- Create: `apps/gui/src/features/receipt/receipt.css`
- Modify: `scripts/check-busytok-gui-surfaces.sh`

**Interfaces:**
- Produces: `.receipt-stage`, `.receipt-paper`, `.receipt-*` classes; scoped `@font-face` for `BusytokMono` + `BusytokSans`.

- [ ] **Step 1: Fetch the OFL font files (Latin subset woff2)**

```bash
mkdir -p apps/gui/public/fonts
# JetBrains Mono + Geist — both SIL OFL 1.1. Bundle under public/ so Vite serves
# them at /fonts/... (absolute path bypasses base:"./"), and modern-screenshot
# fetches that same URL to inline the font into the exported PNG.
curl -L -o apps/gui/public/fonts/JetBrainsMono-Regular.woff2 \
  "https://github.com/JetBrains/JetBrainsMono/raw/master/web/JetBrainsMono-Regular.woff2"
curl -L -o apps/gui/public/fonts/JetBrainsMono-Bold.woff2 \
  "https://github.com/JetBrains/JetBrainsMono/raw/master/web/JetBrainsMono-Bold.woff2"
curl -L -o apps/gui/public/fonts/Geist-Regular.woff2 \
  "https://github.com/vercel/geist-font/raw/main/packages/next/dist/fonts/geist-sans/Geist-Regular.woff2"
curl -L -o apps/gui/public/fonts/Geist-Bold.woff2 \
  "https://github.com/vercel/geist-font/raw/main/packages/next/dist/fonts/geist-sans/Geist-Bold.woff2"
# OFL redistribution requires shipping the license text alongside the fonts:
curl -L -o apps/gui/public/fonts/LICENSE-fonts.txt \
  "https://raw.githubusercontent.com/JetBrains/JetBrainsMono/master/OFL.txt"
ls -la apps/gui/public/fonts/
# If the repo maintains a THIRD_PARTY_NOTICES.md / credits file, add a line
# referencing "JetBrains Mono + Geist, SIL OFL 1.1 — see public/fonts/LICENSE-fonts.txt".
```
(If a URL 404s, grab the equivalent latin-subset woff2 from the latest release of each repo — the exact filenames are what `@font-face` references below. Both fonts are OFL, safe to bundle and redistribute.)

- [ ] **Step 2: Write `receipt.css` — the standalone receipt design language**

Create `apps/gui/src/features/receipt/receipt.css`. This is the showpiece — pursue extreme craft. It is intentionally outside `DESIGN-SYSTEM.md` (own palette/radii):

```css
/* ── Receipt share image — standalone design language ───────────────────
   NOT subject to DESIGN-SYSTEM.md. Excluded from check-busytok-gui-surfaces.sh
   via --glob '!**/receipt.css'. Bundled fonts scoped to .receipt-paper so app
   chrome (system fonts) is untouched. */

@font-face {
  font-family: "BusytokMono";
  src: url("/fonts/JetBrainsMono-Regular.woff2") format("woff2");
  font-weight: 400;
  font-style: normal;
  font-display: block;
}
@font-face {
  font-family: "BusytokMono";
  src: url("/fonts/JetBrainsMono-Bold.woff2") format("woff2");
  font-weight: 700;
  font-style: normal;
  font-display: block;
}
@font-face {
  font-family: "BusytokSans";
  src: url("/fonts/Geist-Regular.woff2") format("woff2");
  font-weight: 400;
  font-style: normal;
  font-display: block;
}
@font-face {
  font-family: "BusytokSans";
  src: url("/fonts/Geist-Bold.woff2") format("woff2");
  font-weight: 700;
  font-style: normal;
  font-display: block;
}

.receipt {
  --r-ink: #211b14;
  --r-ink-muted: #6b5e4a;
  --r-paper-top: #fffdf6;
  --r-paper-bottom: #f6efe2;
  --r-stage: #e9e4da;
  --r-accent: #b4452f;
  --r-divider: rgba(40, 30, 20, 0.28);

  font-family: "BusytokMono", ui-monospace, monospace;
  color: var(--r-ink);
  font-variant-numeric: tabular-nums;
  letter-spacing: -0.01em;
  -webkit-font-smoothing: antialiased;
}

/* The neutral stage the paper floats on (export background). The radial
   vignette here carries the "floating" depth in the exported PNG — it is a
   background-image, which modern-screenshot captures reliably. (.receipt-paper
   keeps only a modest box-shadow: large blur × scale corrupts on WebKit
   foreignObject — modern-screenshot#49.) */
.receipt-stage {
  display: flex;
  justify-content: center;
  padding: 64px 48px;
  background:
    radial-gradient(70% 50% at 50% 44%, rgba(0, 0, 0, 0.12), transparent 72%),
    radial-gradient(120% 80% at 50% 0%, rgba(0, 0, 0, 0.05), transparent 60%),
    var(--r-stage);
  border-radius: 24px;
}

/* The paper: fixed export width, height auto. */
.receipt-paper {
  position: relative;
  width: 420px;
  padding: 40px 36px 44px;
  border-radius: 14px 14px 0 0; /* rounded top; bottom carries the SVG scallop */
  background:
    radial-gradient(circle at 18% 8%, rgba(0, 0, 0, 0.03), transparent 22%),
    linear-gradient(180deg, var(--r-paper-top) 0%, var(--r-paper-bottom) 100%);
  /* Modest shadow only — the stage vignette carries depth (capture-safe).
     Large blur × scale corrupts on WebKit foreignObject (modern-screenshot#49). */
  box-shadow:
    0 12px 30px rgba(35, 25, 15, 0.16),
    inset 0 0 0 1px rgba(60, 40, 20, 0.08);
}

/* Capture-safe scalloped bottom edge: an inline <svg> (rendered in
   ReceiptPaper) survives modern-screenshot's foreignObject capture, unlike a
   CSS `mask` (which the foreignObject approach drops). */
.receipt__tear {
  position: absolute;
  left: 0;
  bottom: -13px;
  width: 420px;
  height: 14px;
  display: block;
}

/* Faint paper grain noise. */
.receipt-paper::after {
  content: "";
  position: absolute;
  inset: 0;
  pointer-events: none;
  opacity: 0.12;
  mix-blend-mode: multiply;
  background-image: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='160' height='160'><filter id='n'><feTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='2' stitchTiles='stitch'/></filter><rect width='100%25' height='100%25' filter='url(%23n)' opacity='0.5'/></svg>");
}

.receipt__header {
  position: relative;
  text-align: center;
  margin-bottom: 6px;
}
.receipt__brand {
  font-family: "BusytokSans", system-ui, sans-serif;
  font-weight: 700;
  font-size: 30px;
  letter-spacing: 0.34em;
  text-indent: 0.34em; /* visually center despite tracking */
  line-height: 1;
}
.receipt__brand::after {
  content: "";
  display: block;
  width: 44px;
  height: 3px;
  margin: 12px auto 0;
  background: var(--r-accent);
  border-radius: 2px;
}
.receipt__subtitle {
  margin-top: 12px;
  font-size: 10.5px;
  letter-spacing: 0.22em;
  color: var(--r-ink-muted);
}

.receipt__divider {
  height: 0;
  border-top: 1px dashed var(--r-divider);
  margin: 18px 0;
}

.receipt__meta {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  gap: 4px 16px;
  font-size: 11px;
  color: var(--r-ink-muted);
}

.receipt__hero {
  text-align: center;
  padding: 8px 0 4px;
}
.receipt__hero-value {
  font-family: "BusytokMono";
  font-weight: 700;
  font-size: 52px;
  line-height: 1.02;
  letter-spacing: -0.02em;
}
.receipt__hero-label {
  margin-top: 6px;
  font-size: 11px;
  letter-spacing: 0.22em;
  color: var(--r-ink-muted);
}
.receipt__hero-secondary {
  margin-top: 12px;
  font-size: 11.5px;
  color: var(--r-ink-muted);
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.receipt__items-header {
  font-size: 11px;
  letter-spacing: 0.2em;
  color: var(--r-ink-muted);
  margin-bottom: 10px;
}
.receipt__item {
  display: grid;
  grid-template-columns: 1fr auto;
  align-items: baseline;
  gap: 0 10px;
  font-size: 13px;
  margin-bottom: 9px;
}
.receipt__item-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* Leader dots between name and the value column. */
.receipt__item-name::after {
  content: " . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .";
  letter-spacing: 2px;
  color: var(--r-divider);
}
.receipt__item-value {
  display: flex;
  gap: 12px;
  align-items: baseline;
}
.receipt__item-tokens {
  font-weight: 700;
}
.receipt__item-cost {
  color: var(--r-ink-muted);
  min-width: 64px;
  text-align: right;
}
.receipt__item--others .receipt__item-name {
  color: var(--r-ink-muted);
  font-style: italic;
}

.receipt__total {
  display: grid;
  grid-template-columns: 1fr auto;
  align-items: baseline;
  gap: 0 10px;
  margin-top: 4px;
  padding-top: 14px;
  border-top: 3px double var(--r-ink);
  font-size: 16px;
  font-weight: 700;
}
.receipt__total-value {
  display: flex;
  gap: 14px;
  align-items: baseline;
}
.receipt__total-cost {
  color: var(--r-accent);
}

.receipt__stamp {
  position: absolute;
  right: 28px;
  bottom: 96px;
  transform: rotate(-12deg);
  border: 2px solid var(--r-accent);
  color: var(--r-accent);
  border-radius: 999px;
  padding: 6px 14px;
  font-family: "BusytokSans", system-ui, sans-serif;
  font-weight: 700;
  font-size: 11px;
  letter-spacing: 0.18em;
  opacity: 0.72;
}

.receipt__footer {
  position: relative;
  margin-top: 26px;
  text-align: center;
  font-size: 10px;
  letter-spacing: 0.18em;
  color: var(--r-ink-muted);
  line-height: 1.9;
}
.receipt__barcode {
  margin: 14px auto 0;
  height: 34px;
  width: 70%;
  background: repeating-linear-gradient(
    90deg,
    var(--r-ink) 0 1px,
    transparent 1px 3px,
    var(--r-ink) 3px 4px,
    transparent 4px 6px,
    var(--r-ink) 6px 9px,
    transparent 9px 12px
  );
  opacity: 0.78;
}

/* The hidden, off-screen export target: rendered at exactly 420px, captured
   by modern-screenshot. NOT display:none (capture libs skip those). */
.receipt-export-root {
  position: fixed;
  left: -10000px;
  top: 0;
  width: 420px;
  pointer-events: none;
}
```

- [ ] **Step 3: Exclude `receipt.css` from the design-system guard**

In `scripts/check-busytok-gui-surfaces.sh`, add `--glob '!**/receipt.css'` to the three CSS `rg` invocations:

```diff
-if rg -n -e 'border-radius:[[:space:]]*(18|20|22|24|26|32)px' apps/gui/src --glob '*.css'; then
+if rg -n -e 'border-radius:[[:space:]]*(18|20|22|24|26|32)px' apps/gui/src --glob '*.css' --glob '!**/receipt.css'; then
```
```diff
 if rg -n -e '--color-surface-strong|--color-surface-elevated|--color-canvas-subtle|--color-border-soft|--color-sidebar|--radius-xs|--radius-xl' \
-  apps/gui/src --glob '!**/tokens.css' --glob '!**/tokens.test.ts'; then
+  apps/gui/src --glob '!**/tokens.css' --glob '!**/tokens.test.ts' --glob '!**/receipt.css'; then
```
```diff
-if rg -n --glob '*.css' --glob '!tokens.css' -e '#[0-9a-fA-F]{3,8}' apps/gui/src/styles; then
+if rg -n --glob '*.css' --glob '!tokens.css' --glob '!**/receipt.css' -e '#[0-9a-fA-F]{3,8}' apps/gui/src/styles; then
```
(File-level exclusion only — do NOT exclude the whole `features/receipt/` subtree; that would silence the guard for `.ts`/`.tsx` too.)

- [ ] **Step 4: Verify the guard + commit**

```bash
bash scripts/check-busytok-gui-surfaces.sh && echo "guard ok"
git add apps/gui/public/fonts apps/gui/src/features/receipt/receipt.css scripts/check-busytok-gui-surfaces.sh
git commit -m "feat(gui): standalone receipt design language + bundled fonts + guard exclusion"
```

---

### Task 7: `ReceiptPaper` component + ViewModel + fixtures + tests

**Files:**
- Modify: `apps/gui/src/lib/formatters.ts` (export `formatCostValue`)
- Create: `apps/gui/src/features/receipt/viewModel.ts`
- Create: `apps/gui/src/features/receipt/fixtures.ts`
- Create: `apps/gui/src/features/receipt/ReceiptPaper.tsx`
- Create: `apps/gui/src/features/receipt/ReceiptPaper.test.tsx`

**Interfaces:**
- Produces: `ReceiptViewModel`, `toReceiptViewModel(dto)`, `<ReceiptPaper vm={...} />` (one component; the dialog preview + off-screen export root both render it).
- Consumes: `ReceiptDailyDto` (generated), `formatCompactNumber`, `formatCacheHitRate`, `formatCostValue`.

- [ ] **Step 1: Export `formatCostValue` from `formatters.ts`**

In `apps/gui/src/lib/formatters.ts`, change the `formatCostValue` declaration (around line 13) from `function formatCostValue(...)` to `export function formatCostValue(...)`. (One-word reuse; no behavior change — `formatCost` still calls it.)

- [ ] **Step 2: Write `viewModel.ts`**

Create `apps/gui/src/features/receipt/viewModel.ts`:

```ts
import type {
  CostStatusDto,
  ReceiptDailyDto,
  ReceiptModelSliceDto,
} from "@busytok/protocol-types";
import {
  formatCacheHitRate,
  formatCompactNumber,
  formatCostValue,
} from "../../lib/formatters";

const TOP_N = 5;

export interface ReceiptItem {
  name: string;
  tokens: string;
  cost: string; // "$24.10" | "≈$24.10" | "—"
  others: boolean;
}

export interface ReceiptViewModel {
  dateLabel: string;
  timezone: string;
  hero: { totalTokens: string; totalTokensRaw: number };
  secondary: {
    split: string; // "in 2.1M · out 0.9M · cache 1.8M"
    cost: string; // "est. $47.21" | "est. —"
    cacheHitRate: string;
  };
  items: ReceiptItem[];
  total: { tokens: string; cost: string };
  sessionCount: number;
  eventCount: number;
  peakHour: string | null;
  serial: string; // "#0626-A3F2"
}

function formatReceiptCost(costUsd: number | null, status: CostStatusDto): string {
  if (status === "unavailable" || costUsd === null) return "—";
  // formatCostValue ALREADY returns a "$X.XX" string — do NOT re-prefix "$"
  // (double-"$" bug: would render "$$24.10").
  const value = formatCostValue(costUsd);
  return status === "partial" ? `≈${value}` : value;
}

function worstStatus(rows: ReceiptModelSliceDto[]): CostStatusDto {
  if (rows.length === 0) return "unavailable";
  if (rows.every((r) => r.cost_status === "exact")) return "exact";
  if (rows.every((r) => r.cost_status === "unavailable")) return "unavailable";
  return "partial"; // mixed exact + partial/unavailable
}

function toItem(m: ReceiptModelSliceDto): ReceiptItem {
  return {
    name: m.name,
    tokens: formatCompactNumber(m.tokens),
    cost: formatReceiptCost(m.cost_usd, m.cost_status),
    others: false,
  };
}

function receiptSerial(date: string): string {
  // Deterministic, date-derived pseudo-serial for receipt authenticity.
  const digits = date.replace(/-/g, "").slice(4); // MMDD
  const hash = (date + "busytok")
    .split("")
    .reduce((acc, c) => (acc * 31 + c.charCodeAt(0)) >>> 0, 7);
  const suffix = hash.toString(16).toUpperCase().slice(0, 4).padStart(4, "0");
  return `#${digits}-${suffix}`;
}

export function toReceiptViewModel(dto: ReceiptDailyDto): ReceiptViewModel {
  const m = dto.metrics;
  const ranked = [...dto.top_models].sort((a, b) => b.tokens - a.tokens);
  const top = ranked.slice(0, TOP_N);
  const rest = ranked.slice(TOP_N);

  const items: ReceiptItem[] = top.map(toItem);
  if (rest.length > 0) {
    const othersTokens = rest.reduce((s, r) => s + r.tokens, 0);
    const othersCostUsd = rest.reduce<number>((s, r) => s + (r.cost_usd ?? 0), 0);
    items.push({
      name: `OTHERS (${rest.length})`,
      tokens: formatCompactNumber(othersTokens),
      cost: formatReceiptCost(othersCostUsd, worstStatus(rest)),
      others: true,
    });
  }

  const estCost = `est. ${formatReceiptCost(m.cost_usd, m.cost_status)}`;

  return {
    dateLabel: dto.date_label,
    timezone: dto.timezone,
    hero: {
      totalTokens: formatCompactNumber(m.total_tokens),
      totalTokensRaw: m.total_tokens,
    },
    secondary: {
      split: `in ${formatCompactNumber(m.input_tokens)} · out ${formatCompactNumber(
        m.output_tokens,
      )} · cache ${formatCompactNumber(m.cache_read_tokens)}`,
      cost: estCost,
      cacheHitRate: formatCacheHitRate(m.cache_hit_rate),
    },
    items,
    total: {
      tokens: formatCompactNumber(m.total_tokens),
      cost: formatReceiptCost(m.cost_usd, m.cost_status),
    },
    sessionCount: m.session_count,
    eventCount: m.event_count,
    peakHour: m.peak_hour?.label ?? null,
    serial: receiptSerial(dto.date),
  };
}
```

- [ ] **Step 3: Write `fixtures.ts`**

Create `apps/gui/src/features/receipt/fixtures.ts`:

```ts
import type { ReceiptDailyDto } from "@busytok/protocol-types";

export const NORMAL_DAY: ReceiptDailyDto = {
  date: "2026-06-26",
  date_label: "FRI · JUN 26, 2026",
  timezone: "Asia/Shanghai",
  metrics: {
    total_tokens: 3_412_888, input_tokens: 2_100_000, output_tokens: 912_888,
    cache_read_tokens: 1_800_000, cache_creation_tokens: 60_000,
    cache_hit_rate: 0.4615, cost_usd: 47.21, cost_status: "exact",
    event_count: 312, session_count: 14,
    peak_hour: { label: "14:00", tokens: 612_000 },
  },
  top_models: [
    { name: "claude-sonnet-4-5", tokens: 1_820_442, cost_usd: 24.1, cost_status: "exact" },
    { name: "claude-haiku-4-5", tokens: 810_200, cost_usd: 1.55, cost_status: "exact" },
    { name: "gpt-5.1", tokens: 530_000, cost_usd: 18.4, cost_status: "exact" },
    { name: "deepseek-v3.2", tokens: 252_246, cost_usd: 3.16, cost_status: "exact" },
  ],
  brand: { name: "BUSYTOK", tagline: "AI CODING · TOKEN RECEIPT", github: "github.com/BalianWang/busytok", generated_at_ms: 1_781_600_000_000 },
};

export const MANY_MODELS: ReceiptDailyDto = {
  ...NORMAL_DAY,
  top_models: [
    ...NORMAL_DAY.top_models,
    { name: "gemini-2.5-pro", tokens: 200_000, cost_usd: 2.0, cost_status: "exact" },
    { name: "model-six", tokens: 150_000, cost_usd: 1.0, cost_status: "exact" },
    { name: "model-seven", tokens: 100_000, cost_usd: 0.5, cost_status: "exact" },
    { name: "model-eight", tokens: 50_000, cost_usd: 0.1, cost_status: "exact" },
  ],
};

export const PARTIAL_COST: ReceiptDailyDto = {
  ...NORMAL_DAY,
  metrics: { ...NORMAL_DAY.metrics, cost_status: "partial" },
  top_models: [
    { name: "claude-sonnet-4-5", tokens: 1_820_442, cost_usd: 24.1, cost_status: "exact" },
    { name: "no-price-model", tokens: 810_200, cost_usd: null, cost_status: "unavailable" },
  ],
};

export const ZERO_COST: ReceiptDailyDto = {
  ...NORMAL_DAY,
  metrics: { ...NORMAL_DAY.metrics, cost_usd: null, cost_status: "unavailable", cache_hit_rate: null },
  top_models: [{ name: "free-local-model", tokens: 12_000, cost_usd: null, cost_status: "unavailable" }],
};

export const LONG_NAMES: ReceiptDailyDto = {
  ...NORMAL_DAY,
  top_models: [
    { name: "claude-sonnet-4-5-thinking-very-long-identifier-2026", tokens: 1_000_000, cost_usd: 10, cost_status: "exact" },
    { name: "another-extremely-long-and-descriptive-model-name", tokens: 500_000, cost_usd: 5, cost_status: "exact" },
  ],
};

export const NO_DATA: ReceiptDailyDto = {
  date: "2026-06-26",
  date_label: "FRI · JUN 26, 2026",
  timezone: "UTC",
  metrics: {
    total_tokens: 0, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0,
    cache_creation_tokens: 0, cache_hit_rate: null, cost_usd: null,
    cost_status: "unavailable", event_count: 0, session_count: 0, peak_hour: null,
  },
  top_models: [],
  brand: { name: "BUSYTOK", tagline: "AI CODING · TOKEN RECEIPT", github: "github.com/BalianWang/busytok", generated_at_ms: 0 },
};

// >5 models where the overflow (models 6-7) are ALL unavailable → the OTHERS
// row's aggregate cost_status must be "unavailable" (render "—"), not "partial".
export const OTHERS_ALL_UNAVAILABLE: ReceiptDailyDto = {
  ...NORMAL_DAY,
  top_models: [
    { name: "m1", tokens: 500, cost_usd: 5, cost_status: "exact" },
    { name: "m2", tokens: 400, cost_usd: 4, cost_status: "exact" },
    { name: "m3", tokens: 300, cost_usd: 3, cost_status: "exact" },
    { name: "m4", tokens: 250, cost_usd: 2.5, cost_status: "exact" },
    { name: "m5", tokens: 200, cost_usd: 2, cost_status: "exact" },
    { name: "free-a", tokens: 150, cost_usd: null, cost_status: "unavailable" },
    { name: "free-b", tokens: 120, cost_usd: null, cost_status: "unavailable" },
  ],
};
```

- [ ] **Step 4: Write the failing component test**

Create `apps/gui/src/features/receipt/ReceiptPaper.test.tsx`:

```tsx
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ReceiptPaper } from "./ReceiptPaper";
import { toReceiptViewModel } from "./viewModel";
import {
  MANY_MODELS,
  LONG_NAMES,
  NO_DATA,
  NORMAL_DAY,
  OTHERS_ALL_UNAVAILABLE,
  PARTIAL_COST,
} from "./fixtures";

afterEach(() => cleanup());

function renderVm(dto = NORMAL_DAY) {
  return render(<ReceiptPaper vm={toReceiptViewModel(dto)} />);
}

describe("ReceiptPaper", () => {
  it("renders brand, hero, and a TOTAL block", () => {
    renderVm();
    expect(screen.getByText("BUSYTOK")).toBeDefined();
    expect(screen.getByText("TOTAL TOKENS")).toBeDefined();
    expect(screen.getByText("ITEMS")).toBeDefined();
    expect(screen.getByText("TOTAL")).toBeDefined();
  });

  it("renders an OTHERS row when more than 5 models", () => {
    renderVm(MANY_MODELS);
    expect(screen.getByText(/OTHERS \(3\)/)).toBeDefined();
  });

  it("renders OTHERS as — (not ≈$0.00) when all overflow models are unavailable", () => {
    renderVm(OTHERS_ALL_UNAVAILABLE); // top 5 exact; overflow (2) all unavailable
    expect(screen.getAllByText("—").length).toBe(1); // the OTHERS row cost only
  });

  it("marks partial aggregate cost with ≈ and keeps exact item cost plain", () => {
    renderVm(PARTIAL_COST); // aggregate cost_status partial (47.21); one exact item ($24.10)
    expect(screen.getByText("≈$47.21")).toBeDefined(); // TOTAL block
    expect(screen.getByText("$24.10")).toBeDefined(); // exact-status item row
    expect(screen.getAllByText("—").length).toBeGreaterThan(0); // unavailable item
  });

  it("shows the empty state when there are no models and no tokens", () => {
    const { container } = renderVm(NO_DATA);
    expect(container.querySelector(".receipt-paper__empty")).not.toBeNull();
  });

  it("truncates long model names", () => {
    renderVm(LONG_NAMES);
    const name = screen.getByText(/claude-sonnet-4-5-thinking-very-long/);
    expect(name).toBeDefined();
  });
});
```

- [ ] **Step 5: Run to verify it fails**

```bash
pnpm --filter @busytok/gui test -- ReceiptPaper
```
Expected: FAIL — component not found.

- [ ] **Step 6: Implement `ReceiptPaper.tsx`**

Create `apps/gui/src/features/receipt/ReceiptPaper.tsx`:

```tsx
import type { ReceiptViewModel } from "./viewModel";
import "./receipt.css";

interface ReceiptPaperProps {
  vm: ReceiptViewModel;
}

export function ReceiptPaper({ vm }: ReceiptPaperProps) {
  const empty = vm.hero.totalTokensRaw === 0 && vm.items.length === 0;
  return (
    <div className="receipt-stage">
      <div className="receipt-paper">
        <header className="receipt__header">
          <div className="receipt__brand">BUSYTOK</div>
          <div className="receipt__subtitle">AI CODING · TOKEN RECEIPT</div>
        </header>

        <div className="receipt__divider" />

        <div className="receipt__meta">
          <span>{vm.dateLabel}</span>
          <span>{vm.peakHour ? `PEAK ${vm.peakHour}` : "PEAK —"}</span>
          <span>RECEIPT {vm.serial}</span>
          <span>TZ {vm.timezone}</span>
        </div>

        {empty ? (
          <div className="receipt-paper__empty">No usage recorded for this day.</div>
        ) : (
          <>
            <div className="receipt__divider" />
            <section className="receipt__hero">
              <div className="receipt__hero-value">{vm.hero.totalTokens}</div>
              <div className="receipt__hero-label">TOTAL TOKENS</div>
              <div className="receipt__hero-secondary">
                <span>{vm.secondary.split}</span>
                <span>{vm.secondary.cost} · cache hit {vm.secondary.cacheHitRate}</span>
              </div>
            </section>

            <div className="receipt__divider" />
            <section>
              <div className="receipt__items-header">ITEMS</div>
              {vm.items.map((item) => (
                <div
                  key={item.name}
                  className={`receipt__item${item.others ? " receipt__item--others" : ""}`}
                >
                  <span className="receipt__item-name">{item.name}</span>
                  <span className="receipt__item-value">
                    <span className="receipt__item-tokens">{item.tokens}</span>
                    <span className="receipt__item-cost">{item.cost}</span>
                  </span>
                </div>
              ))}
            </section>

            <div className="receipt__total">
              <span>TOTAL</span>
              <span className="receipt__total-value">
                <span>{vm.total.tokens} tok</span>
                <span className="receipt__total-cost">{vm.total.cost}</span>
              </span>
            </div>

            <div className="receipt__stamp">LOCAL AUDIT</div>
          </>
        )}

        <footer className="receipt__footer">
          <div>NO PROMPTS UPLOADED</div>
          <div>LOCAL AUDIT ONLY</div>
          <div>generated by busytok · /busytok</div>
          <div className="receipt__barcode" aria-hidden="true" />
        </footer>

        {/* Capture-safe scalloped bottom edge (CSS mask is unreliable in
            foreignObject capture). */}
        <svg className="receipt__tear" width="420" height="14" aria-hidden="true">
          <defs>
            <pattern id="receipt-scallop" width="20" height="14" patternUnits="userSpaceOnUse">
              <path d="M0,4 H20 a10,10 0 0,1 -20,0 Z" fill="#f6efe2" />
            </pattern>
          </defs>
          <rect width="420" height="14" fill="url(#receipt-scallop)" />
        </svg>
      </div>
    </div>
  );
}
```

(Add `.receipt-paper__empty { text-align:center; padding: 48px 0; color: var(--r-ink-muted); font-size: 13px; }` to `receipt.css`.)

- [ ] **Step 7: Run tests + typecheck + commit**

```bash
pnpm --filter @busytok/gui test -- ReceiptPaper
pnpm typecheck
git add apps/gui/src/lib/formatters.ts apps/gui/src/features/receipt/viewModel.ts apps/gui/src/features/receipt/fixtures.ts apps/gui/src/features/receipt/ReceiptPaper.tsx apps/gui/src/features/receipt/ReceiptPaper.test.tsx apps/gui/src/features/receipt/receipt.css
git commit -m "feat(gui): ReceiptPaper component + ViewModel + fixtures (cost-honest items, top5/OTHERS, empty state)"
```

---

### Task 8: `useReceiptExport` — capture / copy / save / summary

**Files:**
- Create: `apps/gui/src/features/receipt/useReceiptExport.ts`
- Create: `apps/gui/src/features/receipt/useReceiptExport.test.ts`

**Interfaces:**
- Produces: `useReceiptExport(target, vm, date)` → `{ busy, copyImage, savePng, copySummary }`.
- Consumes: `domToBlob` (modern-screenshot), `writeImage` (clipboard), `save` (dialog), `invoke` (tauri), `reportFrontendEventSafely`.

- [ ] **Step 1: Write the failing test**

Create `apps/gui/src/features/receipt/useReceiptExport.test.ts`:

```ts
import { cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useReceiptExport } from "./useReceiptExport";
import { toReceiptViewModel } from "./viewModel";
import { NORMAL_DAY } from "./fixtures";

const domToBlob = vi.fn();
const writeImage = vi.fn();
const save = vi.fn();
const invoke = vi.fn();
const reportEvent = vi.fn();

vi.mock("modern-screenshot", () => ({ domToBlob: (...a: unknown[]) => domToBlob(...a) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeImage: (...a: unknown[]) => writeImage(...a) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: (...a: unknown[]) => save(...a) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("../../logging/safeReporter", () => ({
  reportFrontendEventSafely: (...a: unknown[]) => reportEvent(...a),
}));

afterEach(() => {
  cleanup();
  domToBlob.mockReset();
  writeImage.mockReset();
  save.mockReset();
  invoke.mockReset();
  reportEvent.mockReset();
});

function blob(bytes: number[]) {
  return new Blob([new Uint8Array(bytes)], { type: "image/png" });
}

describe("useReceiptExport", () => {
  it("copyImage: fonts.ready → domToBlob → writeImage, logs gui.receipt.copied", async () => {
    domToBlob.mockResolvedValue(blob([1, 2, 3]));
    const el = document.createElement("div");
    const { result } = renderHook(() =>
      useReceiptExport({ current: el }, toReceiptViewModel(NORMAL_DAY), "2026-06-26"),
    );
    await result.current.copyImage();
    expect(domToBlob).toHaveBeenCalledWith(el, expect.objectContaining({ scale: 3 }));
    expect(writeImage).toHaveBeenCalledWith(expect.any(Uint8Array));
    expect(reportEvent).toHaveBeenCalledWith(expect.objectContaining({ event_code: "gui.receipt.copied" }));
  });

  it("savePng: save() → invoke save_receipt_png, logs gui.receipt.exported", async () => {
    domToBlob.mockResolvedValue(blob([9, 9]));
    save.mockResolvedValue("/tmp/x.png");
    const el = document.createElement("div");
    const { result } = renderHook(() =>
      useReceiptExport({ current: el }, toReceiptViewModel(NORMAL_DAY), "2026-06-26"),
    );
    await result.current.savePng();
    expect(save).toHaveBeenCalled();
    expect(invoke).toHaveBeenCalledWith("save_receipt_png", expect.objectContaining({ path: "/tmp/x.png" }));
    expect(reportEvent).toHaveBeenCalledWith(expect.objectContaining({ event_code: "gui.receipt.exported" }));
  });

  it("savePng does nothing when the user cancels the dialog", async () => {
    save.mockResolvedValue(null);
    const el = document.createElement("div");
    const { result } = renderHook(() =>
      useReceiptExport({ current: el }, toReceiptViewModel(NORMAL_DAY), "2026-06-26"),
    );
    await result.current.savePng();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("copySummary writes a text summary", async () => {
    const writeText = vi.fn();
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    const el = document.createElement("div");
    const { result } = renderHook(() =>
      useReceiptExport({ current: el }, toReceiptViewModel(NORMAL_DAY), "2026-06-26"),
    );
    await result.current.copySummary();
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining("Busytok"));
    vi.unstubAllGlobals();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
pnpm --filter @busytok/gui test -- useReceiptExport
```
Expected: FAIL — module not found.

- [ ] **Step 3: Implement `useReceiptExport.ts`**

Create `apps/gui/src/features/receipt/useReceiptExport.ts`:

```ts
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { writeImage } from "@tauri-apps/plugin-clipboard-manager";
import { domToBlob } from "modern-screenshot";
import { useState, type RefObject } from "react";
import { reportFrontendEventSafely } from "../../logging/safeReporter";
import type { ReceiptViewModel } from "./viewModel";

export interface ReceiptExportApi {
  busy: boolean;
  copyImage: () => Promise<void>;
  savePng: () => Promise<void>;
  copySummary: () => Promise<void>;
}

const log = (event_code: string, message: string, details: Record<string, unknown>, level: "INFO" | "ERROR" = "INFO") =>
  reportFrontendEventSafely({ level, event_code, message, details });

export function useReceiptExport(
  target: RefObject<HTMLElement | null>,
  vm: ReceiptViewModel,
  date: string,
): ReceiptExportApi {
  const [busy, setBusy] = useState(false);

  async function captureBytes(): Promise<Uint8Array> {
    const node = target.current;
    if (!node) throw new Error("receipt export target not mounted");
    // Deterministic font + paint: load each face explicitly (document.fonts.ready
    // alone can resolve before a not-yet-referenced face is fetched), then
    // double-rAF so layout/paint commit before the clone is serialized.
    const fonts = document.fonts;
    if (fonts) {
      await Promise.all([
        fonts.load('400 1em "BusytokMono"'),
        fonts.load('700 1em "BusytokMono"'),
        fonts.load('400 1em "BusytokSans"'),
        fonts.load('700 1em "BusytokSans"'),
      ]).catch(() => {});
      await fonts.ready.catch(() => {});
    }
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    // Solid backgroundColor (the stage color) — null/transparent can render
    // black in some WebKit foreignObject paths.
    const blob = await domToBlob(node, { scale: 3, backgroundColor: "#E9E4DA" });
    return new Uint8Array(await blob.arrayBuffer());
  }

  async function run(action: string, fn: () => Promise<void>) {
    setBusy(true);
    try {
      await fn();
    } catch (error) {
      const error_message = error instanceof Error ? error.message : String(error);
      log(`gui.receipt.${action}_failed`, `receipt ${action} failed`, { date, error_message }, "ERROR");
    } finally {
      setBusy(false);
    }
  }

  return {
    busy,
    async copyImage() {
      await run("copied", async () => {
        const bytes = await captureBytes();
        await writeImage(bytes);
        log("gui.receipt.copied", "receipt copied to clipboard", { date });
      });
    },
    async savePng() {
      await run("exported", async () => {
        const path = await save({
          defaultPath: `busytok-receipt-${date}.png`,
          filters: [{ name: "PNG Image", extensions: ["png"] }],
        });
        if (!path) return; // user cancelled
        const bytes = await captureBytes();
        await invoke("save_receipt_png", { path, bytes });
        log("gui.receipt.exported", "receipt saved to file", { date, path });
      });
    },
    async copySummary() {
      await run("summary_copied", async () => {
        const text = [
          "Busytok — daily token receipt",
          vm.dateLabel,
          `Total tokens: ${vm.hero.totalTokens}`,
          vm.secondary.cost,
          `Top: ${vm.items.slice(0, 3).map((i) => i.name).join(", ")}`,
        ].join("\n");
        await navigator.clipboard.writeText(text);
        log("gui.receipt.summary_copied", "receipt summary copied", { date });
      });
    },
  };
}
```

- [ ] **Step 4: Run tests + commit**

```bash
pnpm --filter @busytok/gui test -- useReceiptExport
pnpm typecheck
git add apps/gui/src/features/receipt/useReceiptExport.ts apps/gui/src/features/receipt/useReceiptExport.test.ts
git commit -m "feat(gui): useReceiptExport — capture/copy/save/summary with logging"
```

---

### Task 9: `ReceiptPreviewDialog` — preview + off-screen export root + date control

**Files:**
- Create: `apps/gui/src/features/receipt/ReceiptPreviewDialog.tsx`
- Create: `apps/gui/src/features/receipt/ReceiptPreviewDialog.test.tsx`

**Interfaces:**
- Produces: `<ReceiptPreviewDialog open date onDateChange onClose />`.
- Consumes: `useDailyReceipt`, `toReceiptViewModel`, `ReceiptPaper`, `useReceiptExport`, Radix Dialog.

- [ ] **Step 1: Write the failing test**

Create `apps/gui/src/features/receipt/ReceiptPreviewDialog.test.tsx`:

```tsx
import { cleanup, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ReceiptPreviewDialog } from "./ReceiptPreviewDialog";

vi.mock("../../api/useBusytokData", () => ({
  useDailyReceipt: () => ({
    data: {
      data: {
        date: "2026-06-26",
        date_label: "FRI · JUN 26, 2026",
        timezone: "UTC",
        metrics: {
          total_tokens: 100, input_tokens: 40, output_tokens: 60, cache_read_tokens: 10,
          cache_creation_tokens: 1, cache_hit_rate: 0.2, cost_usd: 1.0, cost_status: "exact",
          event_count: 3, session_count: 1, peak_hour: { label: "10:00", tokens: 100 },
        },
        top_models: [{ name: "m", tokens: 100, cost_usd: 1.0, cost_status: "exact" }],
        brand: { name: "BUSYTOK", tagline: "x", github: "x", generated_at_ms: 0 },
      },
    },
    isLoading: false,
    isError: false,
  }),
}));

afterEach(() => cleanup());

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient();
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("ReceiptPreviewDialog", () => {
  it("renders the receipt + action buttons when open", () => {
    wrap(
      <ReceiptPreviewDialog open date="2026-06-26" onDateChange={vi.fn()} onClose={vi.fn()} />,
    );
    expect(screen.getByText("BUSYTOK")).toBeDefined();
    expect(screen.getByRole("button", { name: /copy image/i })).toBeDefined();
    expect(screen.getByRole("button", { name: /save png/i })).toBeDefined();
    expect(screen.getByLabelText(/receipt date/i)).toBeDefined();
  });

  it("renders nothing when closed", () => {
    const { container } = wrap(
      <ReceiptPreviewDialog open={false} date="2026-06-26" onDateChange={vi.fn()} onClose={vi.fn()} />,
    );
    expect(container.querySelector(".receipt-preview")).toBeNull();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
pnpm --filter @busytok/gui test -- ReceiptPreviewDialog
```
Expected: FAIL — module not found.

- [ ] **Step 3: Implement `ReceiptPreviewDialog.tsx`**

Create `apps/gui/src/features/receipt/ReceiptPreviewDialog.tsx`:

```tsx
import * as Dialog from "@radix-ui/react-dialog";
import { useRef } from "react";
import { useDailyReceipt } from "../../api/useBusytokData";
import { ReceiptPaper } from "./ReceiptPaper";
import { toReceiptViewModel } from "./viewModel";
import { useReceiptExport } from "./useReceiptExport";

interface Props {
  open: boolean;
  date: string;
  onDateChange: (date: string) => void;
  onClose: () => void;
}

export function ReceiptPreviewDialog({ open, date, onDateChange, onClose }: Props) {
  const envelope = useDailyReceipt(date);
  const dto = envelope.data?.data ?? null;
  const vm = dto ? toReceiptViewModel(dto) : null;
  const exportRootRef = useRef<HTMLDivElement>(null);
  const exportApi = useReceiptExport(exportRootRef, vm ?? EMPTY_VM, date);

  return (
    <>
    <Dialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="receipt-preview__overlay" />
        <Dialog.Content className="receipt-preview">
          <Dialog.Title className="receipt-preview__title">Daily receipt</Dialog.Title>
          <Dialog.Description className="receipt-preview__desc">
            Preview your day as a shareable receipt.
          </Dialog.Description>

          <label className="receipt-preview__date">
            <span>Receipt date</span>
            <input
              type="date"
              aria-label="Receipt date"
              value={date}
              onChange={(e) => onDateChange(e.target.value)}
            />
          </label>

          <div className="receipt-preview__scroll">
            {vm ? (
              <div className="receipt-preview__paper">
                {/* Scaled live preview */}
                <ReceiptPaper vm={vm} />
              </div>
            ) : (
              <div className="receipt-preview__loading">Loading…</div>
            )}
          </div>

          <footer className="receipt-preview__actions">
            <button type="button" className="btn btn--secondary" onClick={exportApi.copySummary}>
              Copy summary
            </button>
            <button type="button" className="btn btn--secondary" onClick={exportApi.savePng} disabled={exportApi.busy || !vm}>
              Save PNG
            </button>
            <button type="button" className="btn btn--primary" onClick={exportApi.copyImage} disabled={exportApi.busy || !vm}>
              Copy image
            </button>
            <Dialog.Close asChild>
              <button type="button" className="btn btn--ghost">Close</button>
            </Dialog.Close>
          </footer>

        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
      {/* Off-screen export root — OUTSIDE the dialog content so it is not
          focus-trapped or announced; only the capture reads it. */}
      <div className="receipt-export-root" aria-hidden="true">
        <div ref={exportRootRef}>{vm && <ReceiptPaper vm={vm} />}</div>
      </div>
    </>
  );
}

const EMPTY_VM = toReceiptViewModel({
  date: "1970-01-01",
  date_label: "",
  timezone: "UTC",
  metrics: {
    total_tokens: 0, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0,
    cache_creation_tokens: 0, cache_hit_rate: null, cost_usd: null,
    cost_status: "unavailable", event_count: 0, session_count: 0, peak_hour: null,
  },
  top_models: [],
  brand: { name: "BUSYTOK", tagline: "", github: "", generated_at_ms: 0 },
});
```

Append the dialog chrome CSS to `receipt.css`:

```css
.receipt-preview__overlay {
  position: fixed; inset: 0; background: rgba(17, 24, 39, 0.42);
}
.receipt-preview {
  position: fixed; top: 50%; left: 50%; transform: translate(-50%, -50%);
  width: min(560px, 92vw); max-height: 88vh; display: flex; flex-direction: column;
  background: #fff; border-radius: 16px; padding: 20px; gap: 12px;
  box-shadow: 0 24px 70px rgba(0,0,0,0.3);
}
.receipt-preview__title { font-size: 16px; font-weight: 600; }
.receipt-preview__desc { font-size: 12px; color: #6b7280; }
.receipt-preview__date { display: flex; gap: 8px; align-items: center; font-size: 12px; }
.receipt-preview__scroll { overflow: auto; flex: 1; }
.receipt-preview__paper { transform: scale(0.62); transform-origin: top center; height: 0; padding-top: 62%; }
.receipt-preview__actions { display: flex; gap: 8px; justify-content: flex-end; flex-wrap: wrap; }
```

- [ ] **Step 4: Run tests + commit**

```bash
pnpm --filter @busytok/gui test -- ReceiptPreviewDialog
pnpm typecheck
git add apps/gui/src/features/receipt/ReceiptPreviewDialog.tsx apps/gui/src/features/receipt/ReceiptPreviewDialog.test.tsx apps/gui/src/features/receipt/receipt.css
git commit -m "feat(gui): ReceiptPreviewDialog — live preview + off-screen export root + date control + actions"
```

---

### Task 10: Toolbar integration — extract shared refresh handler, `ShareReceiptButton`, wire Overview, acceptance

**Files:**
- Create: `apps/gui/src/components/desktop/useRefreshClickHandler.ts`
- Modify: `apps/gui/src/components/desktop/useRefreshToolbar.tsx`
- Create: `apps/gui/src/features/receipt/ShareReceiptButton.tsx`
- Create: `apps/gui/src/features/receipt/useReceiptToolbar.tsx`
- Modify: `apps/gui/src/pages/OverviewPage.tsx`

**Interfaces:**
- Produces: `useRefreshClickHandler(surface, onRefresh, isFetching)`, `<ShareReceiptButton onClick />`, `useReceiptToolbar({ surface, onRefresh, isFetching, date, onDateChange, onOpenDialog })`.
- Consumes: `RefreshButton`, `useRegisterPageToolbar`, `reportFrontendEventSafely`, `ReceiptPreviewDialog`, lucide `Share2`.

- [ ] **Step 1: Extract `useRefreshClickHandler.ts`**

Create `apps/gui/src/components/desktop/useRefreshClickHandler.ts` (lifted verbatim from the current `useRefreshToolbar` inline handler so behavior is unchanged):

```ts
import { useCallback } from "react";
import { reportFrontendEventSafely } from "../../logging/safeReporter";

export interface RefreshHandlerOptions {
  surface: string;
  onRefresh: () => Promise<unknown> | unknown;
}

export function useRefreshClickHandler({ surface, onRefresh }: RefreshHandlerOptions) {
  return useCallback(async () => {
    reportFrontendEventSafely({
      level: "INFO",
      event_code: "gui.refresh.requested",
      message: "User requested page refresh from titlebar",
      details: { surface, trigger: "titlebar" },
    });
    try {
      await onRefresh();
      reportFrontendEventSafely({
        level: "INFO",
        event_code: "gui.refresh.succeeded",
        message: "Page refresh completed from titlebar",
        details: { surface, trigger: "titlebar" },
      });
    } catch (error) {
      reportFrontendEventSafely({
        level: "ERROR",
        event_code: "gui.refresh.failed",
        message: "Page refresh failed",
        details: {
          surface,
          trigger: "titlebar",
          error_message: error instanceof Error ? error.message : String(error),
        },
      });
    }
  }, [onRefresh, surface]);
}
```

- [ ] **Step 2: Refactor `useRefreshToolbar.tsx` to use it (delete the inline duplicate)**

In `apps/gui/src/components/desktop/useRefreshToolbar.tsx`, replace the inline `handleRefresh` `useCallback` block with:

```tsx
import { useRefreshClickHandler } from "./useRefreshClickHandler";
// ...
  const handleRefresh = useRefreshClickHandler({ surface, onRefresh });
```

Delete the now-redundant inline handler + its `reportFrontendEventSafely` import (now only used via the extracted hook). Keep the `useMemo` + `useRegisterPageToolbar` lines.

- [ ] **Step 3: Verify the existing refresh tests still pass (behavior unchanged)**

```bash
pnpm --filter @busytok/gui test -- useRefreshToolbar
```
Expected: PASS (the extracted handler preserves the exact logging + try/catch).

- [ ] **Step 4: Create `ShareReceiptButton.tsx`**

Create `apps/gui/src/features/receipt/ShareReceiptButton.tsx`:

```tsx
import { Share2 } from "lucide-react";

interface Props {
  onClick: () => void;
  disabled?: boolean;
}

export function ShareReceiptButton({ onClick, disabled }: Props) {
  return (
    <button
      type="button"
      className="refresh-button"
      onClick={onClick}
      disabled={disabled}
      aria-label="Share daily receipt"
      title="Share daily receipt"
    >
      <Share2 size={14} strokeWidth={1.75} />
    </button>
  );
}
```

- [ ] **Step 5: Create `useReceiptToolbar.tsx`**

Create `apps/gui/src/features/receipt/useReceiptToolbar.tsx`:

```tsx
import { useMemo, useState } from "react";
import { RefreshButton } from "../components/desktop/RefreshButton";
import { useRefreshClickHandler } from "../components/desktop/useRefreshClickHandler";
import { useRegisterPageToolbar } from "../components/desktop/PageToolbarContext";
import { ReceiptPreviewDialog } from "./ReceiptPreviewDialog";
import { ShareReceiptButton } from "./ShareReceiptButton";

export interface ReceiptToolbarOptions {
  surface: string;
  onRefresh: () => Promise<unknown> | unknown;
  isFetching: boolean;
  /** Initial receipt date (today, YYYY-MM-DD). */
  today: string;
}

export function useReceiptToolbar({ surface, onRefresh, isFetching, today }: ReceiptToolbarOptions) {
  const [open, setOpen] = useState(false);
  const [date, setDate] = useState(today);
  const handleRefresh = useRefreshClickHandler({ surface, onRefresh });

  const toolbar = useMemo(
    () => (
      <>
        <ShareReceiptButton onClick={() => setOpen(true)} />
        <RefreshButton onRefresh={handleRefresh} isFetching={isFetching} />
      </>
    ),
    [handleRefresh, isFetching],
  );
  useRegisterPageToolbar(toolbar);

  return (
    <ReceiptPreviewDialog
      open={open}
      date={date}
      onDateChange={setDate}
      onClose={() => setOpen(false)}
    />
  );
}
```

- [ ] **Step 6: Wire it into `OverviewPage.tsx`**

In `apps/gui/src/pages/OverviewPage.tsx`, replace the `useRefreshToolbar({ surface: "overview", onRefresh: refetchSummary, isFetching: summaryFetching })` call with:

```tsx
import { useReceiptToolbar } from "../features/receipt/useReceiptToolbar";
// ...
  const today = new Date().toISOString().slice(0, 10);
  const receiptDialog = useReceiptToolbar({
    surface: "overview",
    onRefresh: refetchSummary,
    isFetching: summaryFetching,
    today,
  });
```

And render `{receiptDialog}` once inside the page's returned JSX (Radix Dialog portals itself, so placement is arbitrary — e.g. at the end of the page root). Remove the now-unused `useRefreshToolbar` import.

- [ ] **Step 7: Add a focused toolbar test**

Create `apps/gui/src/features/receipt/useReceiptToolbar.test.tsx`:

```tsx
import { act, cleanup, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { PageToolbarProvider } from "../components/desktop/PageToolbarContext";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useReceiptToolbar } from "./useReceiptToolbar";

function Harness() {
  const dialog = useReceiptToolbar({
    surface: "overview",
    onRefresh: vi.fn(),
    isFetching: false,
    today: "2026-06-26",
  });
  return (
    <QueryClientProvider client={new QueryClient()}>
      <PageToolbarProvider>
        <div />
        {dialog}
      </PageToolbarProvider>
    </QueryClientProvider>
  );
}

afterEach(() => cleanup());

describe("useReceiptToolbar", () => {
  it("renders a Share button that opens the dialog", async () => {
    render(<Harness />);
    const share = screen.getByRole("button", { name: /share daily receipt/i });
    expect(share).toBeDefined();
    await act(async () => {
      share.click();
    });
    expect(screen.getByText("Daily receipt")).toBeDefined();
  });
});
```

- [ ] **Step 8: Full local gate + acceptance**

```bash
pnpm --filter @busytok/gui test
pnpm typecheck
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
bash scripts/check-busytok-gui-surfaces.sh
COVERAGE_GATE=80 bash scripts/coverage.sh
```
Expected: all green. Then run the app, open Overview, click Share (left of Refresh), confirm the receipt renders, switch date, Copy image + Save PNG + Copy summary, and verify the exported PNG visually (extreme craft check: paper texture, scalloped tear edges, leader dots, oxide-red stamp, barcode, tabular numbers, **cost renders single-`$`** (`$24.10`, `≈$47.21`, `—` — not `$$`), and shadow/depth — modern-screenshot#49 can corrupt box-shadow at scale on macOS; the stage vignette is the capture-safe primary, and if the modest paper shadow still looks off, drop it to `none`).

- [ ] **Step 9: Commit**

```bash
git add apps/gui/src/components/desktop/useRefreshClickHandler.ts apps/gui/src/components/desktop/useRefreshToolbar.tsx apps/gui/src/features/receipt/ShareReceiptButton.tsx apps/gui/src/features/receipt/useReceiptToolbar.tsx apps/gui/src/features/receipt/useReceiptToolbar.test.tsx apps/gui/src/pages/OverviewPage.tsx
git commit -m "feat(gui): ShareReceiptButton + useReceiptToolbar (reuses shared refresh handler); wire Overview toolbar"
```

---

## Self-Review

**Spec coverage** — every spec section maps to a task: §4 architecture/data-flow (Tasks 1–4), §5 `receipt.daily` contract + wiring + data-availability rules (Tasks 1–3), §6 standalone visual language incl. fonts/tear-edges/stamp/top5-OTHERS/cost-honesty (Tasks 6–7), §7 export/share flow + button placement (Tasks 5, 8–10), §8 infra incl. guard exclusion + Rust save command + no fs plugin (Tasks 5–6), §9 testing incl. +08:00 fixture + Vitest call-order + local-only e2e (Tasks 1, 3, 5, 7–10), §10 phasing (Tasks 1→10). No spec requirement is unassigned.

**Placeholder scan** — no TBD/TODO/"add error handling"; every code step contains the actual code; every test contains actual assertions.

**Type consistency** — `ReceiptDailyDto`/`ReceiptModelSliceDto` field names match across the Rust DTO (Task 2), the assembler (Task 3), the generated TS (Task 2 regen), the client/hook (Task 4), the viewmodel (Task 7), and fixtures (Task 7). `toReceiptViewModel`, `ReceiptPaper`, `useReceiptExport`, `useReceiptToolbar` signatures match their consumers. Store fn names match between Task 1 (define) and Task 3 (call). `save_receipt_png` matches between Task 5 (Rust) and Task 8 (`invoke`). `useRefreshClickHandler` is defined once (Task 10) and consumed by both `useRefreshToolbar` and `useReceiptToolbar`.

**Coverage honesty** — Rust store/runtime/protocol/control receipt code is measured by `coverage.sh` and unit-tested (store incl. the +08:00 consistency regression; pure assembler fully tested). The Tauri `save_receipt_png` command is in `busytok-gui` (excluded from the Rust gate; tested on macOS CI only) — covered by 2 unit tests. TS vitest threshold is 90% locally but JS coverage is not run in CI (pre-existing vitest-on-CI hang); the plan does not claim otherwise. The workspace gate default stays 80 (bumping risks red CI on legacy modules — explicitly out of scope).

**Known follow-ups (not blockers)** — Task 6 Step 1 fetches font woff2 from upstream; if a URL has moved, fetch the equivalent OFL latin-subset woff2 and keep the filenames referenced by `@font-face`. Task 9's preview uses a fixed `transform: scale(0.62)` — refine to taste during visual acceptance.
