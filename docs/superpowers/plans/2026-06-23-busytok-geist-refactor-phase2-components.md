# Busytok Geist Refactor — Phase 2: Component Behavior Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rework the seven GUI component areas (Titlebar → Sidebar → Metric → Overview 3-tier → Charts → Prompt Palette → component-level dark) to consume the Phase 1 token contract and embody the "calm, credible, desktop audit tool" governance — without changing the data contract or adding backend capabilities.

**Architecture:** Behavior changes cascade from the Phase 1 token contract (`tokens.css`) which is already landed. The centerpiece is a **pure Titlebar status view-model** (`deriveTitlebarStatus`) that collapses `shell.status` into one escalatable status — the spec's "adapter, not a parallel health state machine." All other tasks are CSS + small component edits that consume existing tokens/hooks/logging. No new protocol types, no new backend calls.

**Tech Stack:** React 19 + TanStack Query, Radix Popover/Tooltip, lightweight-charts (LiveCurve), Nivo (Trend), Vitest + Testing Library (component tests), `safeReportEvent` (observability), the Phase 1 guard (`check-busytok-gui-surfaces.sh`).

## Global Constraints

(From spec `docs/superpowers/specs/2026-06-22-busytok-geist-refactor-design.md` + Phase 1 contract. Every task implicitly includes these.)

- **Indigo accent + SF Pro font unchanged.** Only component behavior + token consumption change.
- **Titlebar uses `shell.status` as the SOLE status source.** The view-model is a pure adapter — no parallel health state machine, no new DTOs. Popover is **read-only** (detail + existing `open_activity`/`open_settings` nav actions). Payload gaps (e.g. last-event time) are single-listed follow-ups, not invented client-side.
- **Material contract (Phase 1):** chrome-only vibrancy; content surfaces opaque; semantic color only for dot/line/pill/flag — never a whole-card wash. Resting panels border-first; `--material-shadow-elevated` is floating-layer-only.
- **Semantic exception hierarchy:** info/success → dot/chip/inline; warning/degraded → chip + 1px semantic border or rail; danger/blocking → stronger container (semantic border). `--color-status-*-soft` tints only small areas.
- **No dead code.** Removed variants/classes/selectors are deleted everywhere (CSS + TS), no aliases. (Phase 1 removed `chartTokens.borderSoft`; Phase 2 removes the `metric-card--success` visual variant, the dynamic heatmap-legend collapse, the titlebar chip-stack, etc.)
- **Observability** via `safeReportEvent` from `apps/gui/src/logging/reporter.ts` (canonical INFO fire-and-forget) at state-transition/action points; event codes follow `gui.<area>.<event>`.
- **Coverage:** target >90% lines; CI floor 85%. The actual gate `pnpm coverage:gui` enforces lines-only at 90%. Every new TS module ships fully unit-tested; component behavior is tested via Testing Library using the established `vi.mock` + `vi.hoisted` patterns.
- **Reuse existing infra:** `tokens.test.ts`, `scripts/check-busytok-gui-surfaces.sh`, `reporter.ts`, `aggregateLagStatus.ts` thresholds, `chartTokens`, the Phase 1 tokens. No new toolchain.
- **Commit per task.** Focused tests via `cd apps/gui && npx vitest run <path>` (the `pnpm --filter … test -- <path>` form does NOT scope and hangs the suite). Run `pnpm --filter @busytok/gui typecheck`, `pnpm check:gui-surfaces`, `pnpm --filter @busytok/gui build` before each commit. The full suite (`cd apps/gui && npx vitest run`) is green on this branch as of Phase 1.

---

## File Structure

| File | Responsibility | This phase |
|---|---|---|
| `apps/gui/src/components/desktop/titlebarStatus.ts` | **New** — pure view-model: derive single escalatable status from `shell.status` | Created (Task 1) |
| `apps/gui/src/components/desktop/titlebarStatus.test.ts` | **New** — view-model unit tests | Created (Task 1) |
| `apps/gui/src/components/desktop/TitlebarStatusChip.tsx` | **New** — single calm chip + read-only Radix popover | Created (Task 2) |
| `apps/gui/src/components/desktop/statusAction.ts` | **New** — shared `statusActionToPage(action): DesktopPage \| undefined` (unknown → undefined, no fallback) | Created (Task 2); `StatusChip.tsx` refactored to use it |
| `apps/gui/src/components/desktop/TitlebarStatusChip.test.tsx` | **New** — chip + popover rendering | Created (Task 2) |
| `apps/gui/src/components/AppShell.tsx` | Titlebar composition | Wire view-model + single chip (Task 2) |
| `apps/gui/src/components/desktop/Sidebar.tsx` | Nav | Group rename `Primary`→`MONITORING` (Task 3) |
| `apps/gui/src/styles/components.css` | Sidebar/titlebar/chip/palette/table CSS | active rail, hover→`--color-hover`, keycap shadow, etc. (Tasks 2,3,9,10) |
| `apps/gui/src/styles/pages.css` | Overview/metric/heatmap/ranking CSS | metric neutralize, tiers, skeletons, rankings accent (Tasks 4,5,6,8) |
| `apps/gui/src/components/overview/OverviewSummaryPanel.tsx` | Metric cards | Conditional top-accent + tone mapping (Task 4) |
| `apps/gui/src/components/overview/LiveCurvePanel.tsx` | Real-time chart | Remove vertical grid, ≤8% fill, unify chartTokens (Task 7) |
| `apps/gui/src/components/charts/NivoTimelineChart.tsx` | Trend chart | gridX off, gridY 4 (Task 8) |
| `apps/gui/src/components/overview/OverviewRankingsPanel.tsx` | Rankings | `#1`/selected accent class (Task 8) |
| `apps/gui/src/components/overview/OverviewTokenHeatmap.tsx` | Heatmap | Fixed 5-cell legend (Task 8) |
| `apps/gui/src/components/overview/*Panel.tsx` | Overview panels | In-frame skeleton/error states (Task 6) |
| `apps/gui/src/components/overview/PanelSkeleton.tsx` | **New** — shared panel skeleton | Created (Task 6) |
| `apps/gui/src/styles/tokens.css` | Token contract | Add `--space-section-gap` (Task 5) |
| `apps/gui/src/styles/tokens.test.ts` | Contract test | Assert `--space-section-gap` (Task 5) |

**Key reuse points (do not re-create):**
- Escalation thresholds: `aggregateLagStatus.ts` exports `AGGREGATE_LAG_WARNING_THRESHOLD_MS` (5_000) / `AGGREGATE_LAG_CRITICAL_THRESHOLD_MS` (30_000) + `aggregateLagStatusChip` + `formatAggregateLagLabel` — reuse for the view-model.
- Logging: `safeReportEvent(event_code, message, details)` (reporter.ts). For transition telemetry with severity, the existing `aggregateLagStatus.syncAggregateLagTelemetry` is the reference pattern (module-scoped last-seen state + `resetAggregateLagTelemetryStateForTests()`).
- Test mocks: `vi.hoisted(() => ({...}))` + `vi.mock("../api/useBusytokData", ...)` + `vi.mock("../logging/reporter", ...)` (see `AppShellStatus.test.tsx`).
- `StatusActionDto` → `DesktopPage` mapping already exists in `StatusChip.tsx` (`actionToPage`, returns `undefined` for unknown). Task 2 **extracts it to a shared `statusAction.ts`** (`statusActionToPage`) used by both `StatusChip` and `TitlebarStatusChip` — unknown action → `undefined` → no navigation, **no fallback**.

---

### Task 1: Titlebar status view-model (pure adapter)

**Files:**
- Create: `apps/gui/src/components/desktop/titlebarStatus.ts`
- Test: `apps/gui/src/components/desktop/titlebarStatus.test.ts`

**Interfaces:**
- Consumes: `ReadinessStateDto`, `StatusChipDto`, `StatusActionDto` from `@busytok/protocol-types`; `aggregateLagStatusChip` + thresholds from `./aggregateLagStatus`.
- Produces: `deriveTitlebarStatus(input)` → `TitlebarStatus` (see types below); `escalateTone(...)` helper. Consumed by Task 2's `TitlebarStatusChip` + `AppShell`.

- [ ] **Step 1: Write the failing test**

Create `apps/gui/src/components/desktop/titlebarStatus.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { deriveTitlebarStatus, type TitlebarStatusInput } from "./titlebarStatus";

function baseInput(over: Partial<TitlebarStatusInput> = {}): TitlebarStatusInput {
  return {
    readiness: "ready_exact",
    statusChips: [],
    connection: "connected",
    queueDepth: null,
    aggregateLagMs: null,
    generatedAtMs: 1_000,
    ...over,
  };
}

describe("deriveTitlebarStatus", () => {
  it("is neutral/healthy when ready, connected, no queue, no lag, no warning chips", () => {
    const s = deriveTitlebarStatus(baseInput());
    expect(s.tone).toBe("neutral");
    expect(s.label).toBe("Live capture active");
    expect(s.dotToken).toBe("var(--color-status-success)");
    expect(s.auxiliary).toBeUndefined();
  });

  it("escalates to warning on ready_degraded", () => {
    const s = deriveTitlebarStatus(baseInput({ readiness: "ready_degraded" }));
    expect(s.tone).toBe("warning");
    expect(s.label).toBe("Degraded");
  });

  it("escalates to warning on reconnecting (backlog/connection)", () => {
    expect(deriveTitlebarStatus(baseInput({ connection: "reconnecting" })).tone).toBe("warning");
    expect(deriveTitlebarStatus(baseInput({ queueDepth: 42 })).tone).toBe("warning");
  });

  it("escalates to warning on aggregate lag >= warning threshold", () => {
    expect(deriveTitlebarStatus(baseInput({ aggregateLagMs: 6_000 })).tone).toBe("warning");
  });

  it("keeps a perceivable-but-non-blocking warning chip (e.g. budget) as a single warning, NOT +1 danger", () => {
    const s = deriveTitlebarStatus(
      baseInput({ statusChips: [{ id: "budget", label: "Budget at 90%", tone: "warning", detail: null, action: null }] }),
    );
    expect(s.tone).toBe("warning");
    expect(s.auxiliary).toBeUndefined();
  });

  it("adds the +1 danger auxiliary only for an allowlisted blocking chip (scan offline = service down)", () => {
    // The backend (supervisor.rs:1397) emits exactly one danger-tone chip:
    // id "scan" when scan_state == "offline". Only allowlisted ids get +1.
    const s = deriveTitlebarStatus(
      baseInput({ statusChips: [{ id: "scan", label: "Service offline", tone: "danger", detail: "Realtime capture is not running", action: null }] }),
    );
    expect(s.tone).toBe("warning"); // primary stays the consolidated status
    expect(s.auxiliary).toBeDefined();
    expect(s.auxiliary?.tone).toBe("danger");
    expect(s.auxiliary?.label).toBe("Service offline");
  });

  it("does NOT +1 a non-allowlisted danger chip (perceivable-non-blocking stays single warning, no auxiliary)", () => {
    const s = deriveTitlebarStatus(
      baseInput({ statusChips: [{ id: "budget", label: "Budget at 90%", tone: "danger", detail: null, action: null }] }),
    );
    expect(s.auxiliary).toBeUndefined();
  });

  it("exposes read-only popover sections (Service / Live) and existing nav actions only", () => {
    const s = deriveTitlebarStatus(
      baseInput({ readiness: "ready_degraded", connection: "reconnecting", queueDepth: 7, aggregateLagMs: 6_000 }),
    );
    const sectionLabels = s.sections.map((sec) => sec.label);
    expect(sectionLabels).toEqual(["SERVICE", "LIVE"]);
    const live = s.sections.find((sec) => sec.label === "LIVE")!;
    const rowLabels = live.rows.map((r) => r.label);
    expect(rowLabels).toEqual(["Connection", "Queue depth", "Aggregate lag"]);
    // actions are the existing read-only nav actions, nothing new
    expect(s.actions.every((a) => a.action === "open_activity" || a.action === "open_settings")).toBe(true);
  });

  it("label shortens to 'Capture active' fallback via separate field (窄宽)", () => {
    const s = deriveTitlebarStatus(baseInput());
    expect(s.labelShort).toBe("Capture active");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/gui && npx vitest run src/components/desktop/titlebarStatus.test.ts`
Expected: FAIL — `./titlebarStatus` does not exist.

- [ ] **Step 3: Implement the view-model**

Create `apps/gui/src/components/desktop/titlebarStatus.ts`:

```ts
//! titlebarStatus — pure view-model that collapses shell.status into ONE
//! escalatable titlebar status. This is the spec's "adapter, not a parallel
//! health state machine": all inputs come from shell.status; this module only
//! projects them into a single chip + read-only popover + optional +1 danger.

import type {
  ReadinessStateDto,
  StatusActionDto,
  StatusChipDto,
} from "@busytok/protocol-types";

export type TitlebarTone = "neutral" | "warning" | "danger";

export interface TitlebarStatusRow {
  label: string;
  value: string;
}

export interface TitlebarStatusSection {
  label: string;
  rows: TitlebarStatusRow[];
}

export interface TitlebarStatusAction {
  label: string;
  action: StatusActionDto;
}

export interface TitlebarAuxiliary {
  label: string;
  tone: "danger";
  detail: string | null;
}

export interface TitlebarStatus {
  tone: TitlebarTone;
  label: string;
  labelShort: string;
  dotToken: string;
  /** Read-only popover sections. */
  sections: TitlebarStatusSection[];
  /** Existing read-only nav actions only (open_activity / open_settings). */
  actions: TitlebarStatusAction[];
  /** Optional +1 danger entry — only for blocking danger chips. */
  auxiliary: TitlebarAuxiliary | undefined;
  /** Human reason for telemetry. */
  reason: string;
}

export interface TitlebarStatusInput {
  readiness: ReadinessStateDto;
  statusChips: StatusChipDto[];
  connection: "connected" | "reconnecting" | "disconnected";
  queueDepth: number | null;
  aggregateLagMs: number | null;
  generatedAtMs: number | null;
}

const READINESS_LABEL: Record<ReadinessStateDto, string | null> = {
  ready_exact: null,
  ready_degraded: "Degraded",
  rebuilding: "Rebuilding",
  starting: "Starting",
};

function hasWarningChip(chips: StatusChipDto[]): StatusChipDto | undefined {
  return chips.find((c) => c.tone === "warning");
}

// Blocking-danger chip IDs — only these warrant the +1 auxiliary entry.
// "scan" with danger tone = service offline, the one blocking condition the
// backend emits today (supervisor.rs:1397). Explicit allowlist by id so a
// future non-blocking danger chip does NOT silently trigger +1 (spec: only
// service-down / permission / must-decide get +1; perceivable-non-blocking
// issues stay a single warning). Extend this set only when the backend adds a
// new genuinely-blocking danger chip.
const BLOCKING_DANGER_CHIP_IDS = new Set(["scan"]);

function blockingDangerChip(chips: StatusChipDto[]): StatusChipDto | undefined {
  return chips.find((c) => c.tone === "danger" && BLOCKING_DANGER_CHIP_IDS.has(c.id));
}

function formatLag(ms: number | null): string {
  if (ms == null) return "—";
  if (ms >= 1000) return `${(ms / 1000).toFixed(ms >= 10_000 ? 0 : 1)}s`;
  return `${ms}ms`;
}

/**
 * Derive the single titlebar status. Pure: same input → same output, no I/O.
 * Escalation precedence: blocking-danger auxiliary is reported SEPARATELY
 * (auxiliary) while the primary status reflects the consolidated tone.
 */
export function deriveTitlebarStatus(input: TitlebarStatusInput): TitlebarStatus {
  const readinessLabel = READINESS_LABEL[input.readiness];
  const warningChip = hasWarningChip(input.statusChips);
  const dangerChip = blockingDangerChip(input.statusChips);

  const reasons: string[] = [];
  if (readinessLabel) reasons.push(`readiness:${input.readiness}`);
  if (input.connection !== "connected") reasons.push(`connection:${input.connection}`);
  if (input.queueDepth != null && input.queueDepth > 0) reasons.push(`queue:${input.queueDepth}`);
  if (warningChip) reasons.push(`chip:${warningChip.id}`);

  const isWarning =
    readinessLabel != null ||
    input.connection !== "connected" ||
    (input.queueDepth != null && input.queueDepth > 0) ||
    warningChip != null;

  const tone: TitlebarTone = isWarning ? "warning" : "neutral";
  const label = isWarning
    ? readinessLabel ?? (input.connection === "reconnecting" ? "Reconnecting…"
        : input.connection === "disconnected" ? "Disconnected"
        : (input.queueDepth != null && input.queueDepth > 0) ? "Backlog"
        : warningChip?.label ?? "Degraded")
    : "Live capture active";

  const sections: TitlebarStatusSection[] = [
    {
      label: "SERVICE",
      rows: [
        { label: "Readiness", value: readinessLabel ?? "Ready" },
      ],
    },
    {
      label: "LIVE",
      rows: [
        { label: "Connection", value: connectionLabel(input.connection) },
        { label: "Queue depth", value: input.queueDepth != null ? String(input.queueDepth) : "—" },
        { label: "Aggregate lag", value: formatLag(input.aggregateLagMs) },
      ],
    },
  ];

  const actions: TitlebarStatusAction[] = [
    { label: "View Activity", action: "open_activity" },
    { label: "Open Settings", action: "open_settings" },
  ];

  const auxiliary: TitlebarAuxiliary | undefined = dangerChip
    ? { label: dangerChip.label, tone: "danger", detail: dangerChip.detail }
    : undefined;

  return {
    tone,
    label,
    labelShort: isWarning ? label : "Capture active",
    dotToken: tone === "neutral" ? "var(--color-status-success)" : "var(--color-status-warning)",
    sections,
    actions,
    auxiliary,
    reason: reasons.length > 0 ? reasons.join(",") : "healthy",
  };
}

function connectionLabel(c: TitlebarStatusInput["connection"]): string {
  if (c === "connected") return "Connected";
  if (c === "reconnecting") return "Reconnecting";
  return "Disconnected";
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/gui && npx vitest run src/components/desktop/titlebarStatus.test.ts`
Expected: PASS (all 8 tests).

- [ ] **Step 5: Commit**

```bash
git add apps/gui/src/components/desktop/titlebarStatus.ts apps/gui/src/components/desktop/titlebarStatus.test.ts
git commit -m "feat(gui): titlebar status view-model (pure adapter)

deriveTitlebarStatus collapses shell.status into one escalatable status
(neutral→warning, +1 danger auxiliary only for blocking danger chips).
Read-only popover sections (SERVICE/LIVE) + existing nav actions only.
No parallel health state machine; pure projection of shell.status. Phase 2."
```

---

### Task 2: Titlebar single calm chip + read-only popover

**Files:**
- Create: `apps/gui/src/components/desktop/statusAction.ts` (shared `statusActionToPage`)
- Create: `apps/gui/src/components/desktop/TitlebarStatusChip.tsx`, `TitlebarStatusChip.test.tsx`
- Modify: `apps/gui/src/components/desktop/StatusChip.tsx` (use shared `statusActionToPage`, delete local `actionToPage`)
- Modify: `apps/gui/src/components/AppShell.tsx` (replace the chip-stack with one chip)
- Modify: `apps/gui/src/styles/components.css` (calm-chip + popover styling)

**Interfaces:**
- Consumes: `deriveTitlebarStatus` + `TitlebarStatus` from `./titlebarStatus` (Task 1); `useShellStatus` + `useEventSubscription`; `safeReportEvent`.
- Produces: a titlebar that renders exactly ONE primary status chip (+ optional `+1` danger aux) with a read-only popover; emits `gui.titlebar.status_escalated` on tone transitions.

- [ ] **Step 1: Write the failing test**

Create `apps/gui/src/components/desktop/TitlebarStatusChip.test.tsx`:

```tsx
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TitlebarStatusChip } from "./TitlebarStatusChip";
import type { TitlebarStatus } from "./titlebarStatus";

const healthy: TitlebarStatus = {
  tone: "neutral",
  label: "Live capture active",
  labelShort: "Capture active",
  dotToken: "var(--color-status-success)",
  sections: [
    { label: "SERVICE", rows: [{ label: "Readiness", value: "Ready" }] },
    { label: "LIVE", rows: [{ label: "Connection", value: "Connected" }] },
  ],
  actions: [{ label: "View Activity", action: "open_activity" }],
  auxiliary: undefined,
  reason: "healthy",
};

afterEach(cleanup);

describe("TitlebarStatusChip", () => {
  it("renders the single calm chip with a success dot when healthy", () => {
    render(<TitlebarStatusChip status={healthy} onAction={() => {}} />);
    expect(screen.getByRole("button", { name: /Live capture active/ })).toBeDefined();
    expect(screen.queryByText(/Q:/)).toBeNull(); // no queue capsule
  });

  it("renders the auxiliary danger entry beside the primary when present", () => {
    const s: TitlebarStatus = { ...healthy, auxiliary: { label: "Service unreachable", tone: "danger", detail: null } };
    render(<TitlebarStatusChip status={s} onAction={() => {}} />);
    expect(screen.getByRole("button", { name: /Service unreachable/ })).toBeDefined();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/gui && npx vitest run src/components/desktop/TitlebarStatusChip.test.tsx`
Expected: FAIL — `./TitlebarStatusChip` does not exist.

- [ ] **Step 3: Shared action helper + refactor StatusChip + implement the chip**

Create `apps/gui/src/components/desktop/statusAction.ts` (extracts the typed action→page map; unknown → `undefined`, **no fallback**):

```ts
//! statusAction — shared StatusActionDto → DesktopPage mapping. Unknown
//! actions return undefined so callers skip navigation rather than falling
//! back to a default page.
import type { StatusActionDto } from "@busytok/protocol-types";
import type { DesktopPage } from "../AppShell";

export function statusActionToPage(action: StatusActionDto): DesktopPage | undefined {
  switch (action) {
    case "open_activity": return "usage";
    case "open_settings": return "settings";
    default: return undefined;
  }
}
```

Refactor `apps/gui/src/components/desktop/StatusChip.tsx`: delete its local `actionToPage` function, import `statusActionToPage` from `./statusAction`, and replace the call site (`actionToPage(model.action)` → `statusActionToPage(model.action)`). Behavior is identical (same switch, same undefined-for-unknown).

Create `apps/gui/src/components/desktop/TitlebarStatusChip.tsx`:

```tsx
//! TitlebarStatusChip — the ONE calm status entry. Healthy = neutral chip with
//! a success dot; escalates in place (warning/danger) per the view-model. The
//! popover is read-only (sections + existing nav actions). Click emits an
//! acknowledgement event for observability.

import * as Popover from "@radix-ui/react-popover";
import type { DesktopPage } from "../AppShell";
import type { TitlebarStatus } from "./titlebarStatus";
import { statusActionToPage } from "./statusAction";

interface TitlebarStatusChipProps {
  status: TitlebarStatus;
  onAction: (page: DesktopPage) => void;
}

export function TitlebarStatusChip({ status, onAction }: TitlebarStatusChipProps) {
  const toneClass = status.tone === "neutral" ? "is-neutral" : "is-warning";
  return (
    <>
      <Popover.Root>
        <Popover.Trigger asChild>
          <button
            type="button"
            className={`titlebar-chip ${toneClass}`}
            aria-label={status.label}
          >
            <span className="titlebar-chip__dot" style={{ background: status.dotToken }} aria-hidden="true" />
            <span className="titlebar-chip__label">{status.label}</span>
          </button>
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content className="titlebar-popover" sideOffset={8} align="start">
            {status.sections.map((section) => (
              <div key={section.label} className="titlebar-popover__section">
                <p className="titlebar-popover__section-label">{section.label}</p>
                <dl className="titlebar-popover__rows">
                  {section.rows.map((row) => (
                    <div key={row.label} className="titlebar-popover__row">
                      <dt>{row.label}</dt>
                      <dd>{row.value}</dd>
                    </div>
                  ))}
                </dl>
              </div>
            ))}
            {status.actions.length > 0 ? (
              <div className="titlebar-popover__actions">
                {status.actions.map((a) => {
                  const page = statusActionToPage(a.action);
                  if (!page) return null; // unknown action → no button, no navigation (no fallback)
                  return (
                    <button
                      key={a.action}
                      type="button"
                      className="desktop-button desktop-button--small desktop-button--secondary"
                      onClick={() => onAction(page)}
                    >
                      {a.label}
                    </button>
                  );
                })}
              </div>
            ) : null}
            <Popover.Arrow className="titlebar-popover__arrow" />
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>

      {status.auxiliary ? (
        <Popover.Root>
          <Popover.Trigger asChild>
            <button type="button" className="titlebar-chip is-danger" aria-label={status.auxiliary.label}>
              <span className="titlebar-chip__dot" style={{ background: "var(--color-status-danger)" }} aria-hidden="true" />
              <span className="titlebar-chip__label">{status.auxiliary.label}</span>
            </button>
          </Popover.Trigger>
          {status.auxiliary.detail ? (
            <Popover.Portal>
              <Popover.Content className="titlebar-popover" sideOffset={8}>
                <p className="titlebar-popover__detail">{status.auxiliary.detail}</p>
                <Popover.Arrow className="titlebar-popover__arrow" />
              </Popover.Content>
            </Popover.Portal>
          ) : null}
        </Popover.Root>
      ) : null}
    </>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/gui && npx vitest run src/components/desktop/TitlebarStatusChip.test.tsx`
Expected: PASS.

- [ ] **Step 5: Wire into AppShell + add escalation telemetry**

In `apps/gui/src/components/AppShell.tsx`:
- Add imports: `import { deriveTitlebarStatus } from "./desktop/titlebarStatus";` and `import { TitlebarStatusChip } from "./desktop/TitlebarStatusChip";` and `import { safeReportEvent } from "../logging/reporter";`.
- Add a `useRef` to track the previous tone + a `useEffect` emitting escalation telemetry. Replace the entire `desktop-titlebar__status` children (the readiness chip + status_chips map + connection span + queue span + aggregate-lag chip) with:

```tsx
          <TitlebarStatusChip status={status} onAction={onNavigate} />
```

- Build `status` via the adapter where the old derivations were:

```tsx
  const connectionStatus = useEventSubscription().connectionStatus;
  const status = deriveTitlebarStatus({
    readiness: shellStatus?.readiness ?? "starting",
    statusChips: shellStatus?.status_chips?.filter((c) => c.id !== "scan_progress") ?? [],
    connection: connectionStatus,
    queueDepth: shellStatus?.writer_queue_depth ?? null,
    aggregateLagMs: shellStatus?.aggregate_lag_ms ?? null,
    generatedAtMs: shellStatus?.generated_at_ms ?? null,
  });

  const prevToneRef = useRef<TitlebarTone | null>(null);
  useEffect(() => {
    const prev = prevToneRef.current;
    if (prev != null && prev !== status.tone) {
      safeReportEvent(
        "gui.titlebar.status_escalated",
        "Titlebar status tone changed",
        { from: prev, to: status.tone, reason: status.reason },
      );
    }
    prevToneRef.current = status.tone;
  }, [status.tone, status.reason]);
```

(Add `TitlebarTone` to the import from `./titlebarStatus`, and `useEffect, useRef` are already imported. Delete the now-unused local `readinessChip` helper, `VISUALLY_HIDDEN_STYLE`, the `readinessAnnouncement` block, and the `aggregateLagStatusChip` usage in render. **KEEP `syncAggregateLagTelemetry(aggregateLagMs)`** in its own `useEffect` on `aggregateLagMs` — it emits the dedicated `gui.shell.aggregate_lag_warning_visible` / `critical_visible` / `recovered` events with the 5s/30s threshold semantics that the coarser UI-level `gui.titlebar.status_escalated` does NOT capture. The new titlebar event is ADDITIVE (UI consolidation), not a replacement — dual-track observability. Do not remove the lag-specific telemetry.)

- [ ] **Step 6: Add calm-chip + popover CSS**

In `apps/gui/src/styles/components.css`, add (replacing the old `.desktop-titlebar__status`/`.desktop-titlebar__conn-status`/`.desktop-titlebar__queue-depth` rules, which are no longer rendered — delete them):

```css
/* ── Titlebar calm status chip (single entry) ─────────────────── */
.titlebar-chip {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  height: 26px;
  padding: 0 10px;
  border: 1px solid var(--color-border-subtle);
  border-radius: var(--radius-sm);            /* 6px rect, not a pill */
  background: var(--color-surface-subtle);
  color: var(--color-text-muted);
  font-size: 12.5px;
  font-weight: 500;
  cursor: pointer;
  transition: background 120ms ease, border-color 120ms ease;
}
.titlebar-chip:hover { background: var(--color-hover); }
.titlebar-chip:focus-visible { outline: 2px solid var(--color-focus-ring); outline-offset: 2px; }
.titlebar-chip__dot { width: 6px; height: 6px; border-radius: 50%; flex-shrink: 0; }
.titlebar-chip.is-warning {
  background: var(--color-status-warning-soft);
  border-color: color-mix(in srgb, var(--color-status-warning) 30%, transparent);
  color: var(--color-text);
}
.titlebar-chip.is-danger {
  background: var(--color-status-danger-soft);
  border-color: color-mix(in srgb, var(--color-status-danger) 30%, transparent);
  color: var(--color-text);
}

/* Read-only popover (floating layer → elevated shadow) */
.titlebar-popover {
  width: 280px;
  padding: var(--space-3);
  border-radius: var(--radius-md);
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  box-shadow: var(--material-shadow-elevated);
  z-index: 200;
}
.titlebar-popover__section + .titlebar-popover__section { margin-top: var(--space-2); }
.titlebar-popover__section-label {
  margin: 0 0 6px;
  font-size: 11px; font-weight: 600;
  text-transform: uppercase; letter-spacing: 0.04em;
  color: var(--color-text-faint);
}
.titlebar-popover__rows { margin: 0; display: grid; gap: 4px; }
.titlebar-popover__row { display: flex; justify-content: space-between; gap: 12px; margin: 0; }
.titlebar-popover__row dt { color: var(--color-text-muted); font-size: 13px; }
.titlebar-popover__row dd { margin: 0; color: var(--color-text); font-size: 13px; font-variant-numeric: tabular-nums; }
.titlebar-popover__actions { display: flex; gap: var(--space-2); margin-top: var(--space-3); }
.titlebar-popover__detail { margin: 0; color: var(--color-text-muted); font-size: 13px; line-height: 1.45; }
.titlebar-popover__arrow { fill: var(--color-surface); }
```

- [ ] **Step 7: Run focused tests + typecheck + guard + build**

Run: `cd apps/gui && npx vitest run src/components/desktop/TitlebarStatusChip.test.tsx src/components/desktop/titlebarStatus.test.ts src/components/AppShellStatus.test.tsx` — then `pnpm --filter @busytok/gui typecheck && pnpm check:gui-surfaces && pnpm --filter @busytok/gui build`.
Expected: tests PASS (the existing `AppShellStatus.test.tsx` may need its assertions updated to the single-chip model — update expectations in that file to match: one chip, no `Q:`/`⟳` capsules; if it asserts the old stack, rewrite those assertions to the new model).

- [ ] **Step 8: Commit**

```bash
git add apps/gui/src/components/desktop/TitlebarStatusChip.tsx apps/gui/src/components/desktop/TitlebarStatusChip.test.tsx apps/gui/src/components/AppShell.tsx apps/gui/src/components/AppShellStatus.test.tsx apps/gui/src/styles/components.css
git commit -m "feat(gui): titlebar single calm chip + read-only popover

Replaces the 7-input chip stack with one TitlebarStatusChip driven by the
view-model; healthy = neutral chip + success dot, escalates in place, +1
danger auxiliary only for blocking issues. Read-only popover (SERVICE/LIVE
+ existing nav actions). Emits gui.titlebar.status_escalated on tone change.
Removes the queue/connection/lag capsules and the local readinessChip helper."
```

---

### Task 3: Sidebar directory feel

**Files:**
- Modify: `apps/gui/src/components/desktop/Sidebar.tsx` (group rename `Primary` → `MONITORING`)
- Modify: `apps/gui/src/styles/components.css` (active = rail + `--color-hover`-strong; hover = `--color-hover`; item height 32)

**Interfaces:** none new.

- [ ] **Step 1: Rename the first group label**

In `apps/gui/src/components/desktop/Sidebar.tsx`, change the first group's `label: "Primary"` to `label: "Monitoring"` (rendered uppercase by CSS). (Tools/System stay.)

- [ ] **Step 2: Rewrite the sidebar-item states in CSS**

In `apps/gui/src/styles/components.css`, replace the `.desktop-sidebar__item`, `:hover`, `.is-active` rules with:

```css
.desktop-sidebar__item {
  width: 100%;
  height: 32px;                                /* was 36 — more list-like */
  display: flex; align-items: center; gap: 10px;
  border: 0; border-radius: var(--radius-sm);
  background: transparent;
  color: var(--color-text-muted);             /* recedes at rest */
  font-size: 13.5px; padding: 0 12px;
  cursor: pointer; text-align: left;
  position: relative;
  transition: background 120ms ease, color 120ms ease;
}
.desktop-sidebar__item:hover { background: var(--color-hover); color: var(--color-text); }
.desktop-sidebar__item:focus-visible { outline: 2px solid var(--color-focus-ring); outline-offset: -2px; }
/* Active = accent text/icon + 2px left rail + very-subtle neutral support (NO accent tint block). */
.desktop-sidebar__item.is-active {
  background: var(--color-hover-strong);
  color: var(--color-accent-600);
  font-weight: 500;
}
.desktop-sidebar__item.is-active::before {
  content: ""; position: absolute; left: 0; top: 50%; transform: translateY(-50%);
  width: 2px; height: 60%; border-radius: 999px; background: var(--color-accent-500);
}
```

(Delete the old `.desktop-sidebar__item.is-active { background: var(--material-tint-accent); ... }` rule. Dark theme uses `--color-accent-600` which is fine; if dark contrast is weak, override to `--color-accent-400` under `:root[data-theme="dark"] .desktop-sidebar__item.is-active { color: var(--color-accent-400); }`.)

- [ ] **Step 3: Run guard + build + sidebar test (if any) + typecheck**

Run: `pnpm check:gui-surfaces && pnpm --filter @busytok/gui build && pnpm --filter @busytok/gui typecheck && (cd apps/gui && npx vitest run src/components/desktop/Sidebar.test.tsx 2>/dev/null || true)`.
Expected: guard + build + typecheck green; sidebar test (if present) passes or is absent.

- [ ] **Step 4: Commit**

```bash
git add apps/gui/src/components/desktop/Sidebar.tsx apps/gui/src/styles/components.css
git commit -m "refactor(gui): sidebar directory feel — active rail + hover token

Active item = accent text/icon + 2px left rail + very-subtle neutral support
(--color-hover-strong); no accent-tint block. Hover uses --color-hover. Item
height 36→32. First group renamed Primary→Monitoring. Phase 2."
```

---

### Task 4: Metric card neutralization

**Files:**
- Modify: `apps/gui/src/components/overview/OverviewSummaryPanel.tsx` (conditional top-accent + tone mapping)
- Modify: `apps/gui/src/styles/pages.css` (neutral default; delete `--success` visual; warning/danger = flag + dot)

**Interfaces:** consumes `OverviewMetricDto.tone` (`ToneDto`).

- [ ] **Step 1: Make the top-accent conditional + success render as neutral**

In `apps/gui/src/components/overview/OverviewSummaryPanel.tsx`, replace the metric-card map block (currently lines ~45–53) so that: success is treated as neutral (no accent bar, no wash), and the top-accent bar renders only for warning/danger:

```tsx
          {data.metrics.map((metric) => {
            // success reads as neutral (no semantic billboard); only
            // warning/danger carry the exception flag.
            const cardTone: "neutral" | "warning" | "danger" =
              metric.tone === "warning" || metric.tone === "danger" ? metric.tone : "neutral";
            const showsFlag = cardTone === "warning" || cardTone === "danger";
            return (
              <div key={metric.id} className={`metric-card metric-card--${cardTone}`}>
                {showsFlag ? <div className="metric-card__top-accent" aria-hidden="true" /> : null}
                <div className="metric-card__label">{metric.label.toUpperCase()}</div>
                <div className="metric-card__value">{metric.value}</div>
                {metric.helper ? <div className="metric-card__helper">{metric.helper}</div> : null}
              </div>
            );
          })}
```

- [ ] **Step 2: Rewrite the metric-card tone variants in CSS**

In `apps/gui/src/styles/pages.css`, replace the `.metric-card--*` blocks (currently lines ~176–203) with:

```css
/* Default (neutral + success): no wash, no accent bar, border-only. */
.metric-card--neutral {
  background: var(--color-surface);            /* opaque, no tint */
}
/* success is gone as a visual variant — it is rendered as neutral. */

/* Exception cards: 2px top flag only; number + background stay neutral. */
.metric-card--warning .metric-card__top-accent { background: var(--color-status-warning); }
.metric-card--danger  .metric-card__top-accent { background: var(--color-status-danger); }
.metric-card--warning { border-color: var(--color-border-subtle); }   /* keep neutral border; flag carries the signal */
.metric-card--danger  { border-color: color-mix(in srgb, var(--color-status-danger) 45%, var(--color-border-subtle)); } /* stronger semantic container */
```

(Remove the old `--success` block entirely. The base `.metric-card` already has `background: var(--color-surface)` + `border: 1px solid var(--color-border-subtle)` + no shadow — keep that. The `.metric-card__top-accent` base rule stays at 2px.)

- [ ] **Step 3: Run guard + build + overview panel tests**

Run: `pnpm check:gui-surfaces && pnpm --filter @busytok/gui build && (cd apps/gui && npx vitest run src/components/overview/OverviewPanels.test.tsx)`.
Expected: green. (If `OverviewPanels.test.tsx` asserts the `top-accent` is always present or asserts `metric-card--success`, update those assertions to the neutralized model — success→neutral, flag only for warning/danger.)

- [ ] **Step 4: Commit**

```bash
git add apps/gui/src/components/overview/OverviewSummaryPanel.tsx apps/gui/src/styles/pages.css apps/gui/src/components/overview/OverviewPanels.test.tsx
git commit -m "refactor(gui): metric card neutralization

Neutral (incl. success) = opaque surface + border, no wash, no accent bar.
Warning/danger = 2px top flag (+ danger strengthens the border); number and
background never recolor. Removes the metric-card--success visual variant
(success renders as neutral). Top-accent is conditional. Phase 2."
```

---

### Task 5: Overview page shell + 3 tiers

**Files:**
- Modify: `apps/gui/src/styles/tokens.css` (add `--space-section-gap`)
- Modify: `apps/gui/src/styles/tokens.test.ts` (assert it)
- Modify: `apps/gui/src/styles/pages.css` (section-gap, max-width 1600, Tier A/B/C border+surface)

**Interfaces:** adds `--space-section-gap: 24px`.

- [ ] **Step 1: Add + assert the section-gap token**

In `apps/gui/src/styles/tokens.css` light `:root`, in the spacing block, add `--space-section-gap: 24px;`. Add the failing assertion to `tokens.test.ts` (`expect(tokensCss).toContain("--space-section-gap: 24px;");`), run red, then it passes.

Run: `cd apps/gui && npx vitest run src/styles/tokens.test.ts` → PASS after adding the token.

- [ ] **Step 2: Apply page-shell rhythm + tiers in CSS**

In `apps/gui/src/styles/pages.css`:
- `.overview-console`: change `gap: 24px` → `gap: var(--space-section-gap)`; add `max-width: 1600px; margin-inline: auto;`.
- `.overview-console__trend, .live-curve-panel` (Tier A primary): change `border: 1px solid var(--color-border-subtle)` → `border: 1px solid var(--color-border)` (the stronger border). Keep `box-shadow: var(--material-shadow-card)` (Phase 1) — or drop to `none` for border-first; the spec says Tier A is border-first, shadow optional. Set `box-shadow: none` and rely on `--color-border`.
- `.overview-heatmap` (also Tier A): same — `border: 1px solid var(--color-border)`, `box-shadow: none`.
- `.metric-card` (Tier B): already `--color-surface` + `--color-border-subtle` + no shadow — confirm no shadow. (It is.)
- `.ranking-section` and `.overview-console__recent` (Tier C): already `--color-surface-subtle` + `--color-border-subtle` + no shadow — confirm. (They are.)

- [ ] **Step 3: Run guard + build + tokens test**

Run: `cd apps/gui && npx vitest run src/styles/tokens.test.ts && pnpm check:gui-surfaces && pnpm --filter @busytok/gui build`.
Expected: green.

- [ ] **Step 4: Commit**

```bash
git add apps/gui/src/styles/tokens.css apps/gui/src/styles/tokens.test.ts apps/gui/src/styles/pages.css
git commit -m "refactor(gui): overview page shell + 3 tiers

Adds --space-section-gap (24px) for section rhythm; content max-width 1600
centered. Tier A (trend/live/heatmap) = surface + --color-border (strong),
border-first no shadow. Tier B (metrics) = surface + border-subtle. Tier C
(rankings/recent) = surface-subtle + border-subtle. Phase 2."
```

---

### Task 6: Overview state-in-frame (skeleton + inline error + degraded ribbon)

**Files:**
- Create: `apps/gui/src/components/overview/PanelSkeleton.tsx`
- Modify: `apps/gui/src/components/overview/OverviewSummaryPanel.tsx`, `OverviewTrendPanel.tsx`, `OverviewHeatmapPanel.tsx`, `OverviewRankingsPanel.tsx` (loading → in-frame skeleton; error → inline; keep catastrophic full-page PageState only for summary-unavailable)
- Modify: `apps/gui/src/pages/OverviewPage.tsx` (degraded → thin ribbon instead of centered PageState card)
- Modify: `apps/gui/src/styles/pages.css` (skeleton + ribbon styles)

**Interfaces:** `PanelSkeleton` props: `{ variant: "metrics" | "chart" | "table" | "list"; rows?: number }`.

- [ ] **Step 1: Create the shared skeleton component**

Create `apps/gui/src/components/overview/PanelSkeleton.tsx`:

```tsx
//! PanelSkeleton — a low-contrast in-frame placeholder so the panel frame
//! stays stable during loading (no layout jump, no full-page spinner card).

interface PanelSkeletonProps {
  variant: "metrics" | "chart" | "table" | "list";
  rows?: number;
}

export function PanelSkeleton({ variant, rows = 3 }: PanelSkeletonProps) {
  if (variant === "metrics") {
    return (
      <div className="panel-skeleton panel-skeleton--metrics" aria-hidden="true">
        {[0, 1, 2].map((i) => (
          <div key={i} className="panel-skeleton__metric">
            <div className="panel-skeleton__bar panel-skeleton__bar--label" />
            <div className="panel-skeleton__bar panel-skeleton__bar--value" />
            <div className="panel-skeleton__bar panel-skeleton__bar--helper" />
          </div>
        ))}
      </div>
    );
  }
  if (variant === "chart") {
    return (
      <div className="panel-skeleton panel-skeleton--chart" aria-hidden="true">
        <div className="panel-skeleton__curve" />
      </div>
    );
  }
  // table / list
  return (
    <div className="panel-skeleton panel-skeleton--rows" aria-hidden="true">
      {Array.from({ length: rows }).map((_, i) => (
        <div key={i} className="panel-skeleton__row" />
      ))}
    </div>
  );
}
```

- [ ] **Step 2: Add skeleton + degraded-ribbon CSS**

In `apps/gui/src/styles/pages.css`:

```css
/* ── Panel skeletons (in-frame loading; frame stays stable) ─────── */
.panel-skeleton { opacity: 0.55; }
.panel-skeleton--metrics { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; }
.panel-skeleton__metric { display: grid; gap: 8px; padding: 16px 18px; border: 1px solid var(--color-border-subtle); border-radius: var(--radius-md); }
.panel-skeleton__bar { background: var(--color-surface-subtle); border-radius: 4px; }
.panel-skeleton__bar--label { width: 40%; height: 10px; }
.panel-skeleton__bar--value { width: 70%; height: 22px; }
.panel-skeleton__bar--helper { width: 55%; height: 10px; }
.panel-skeleton--chart { height: 220px; display: flex; align-items: end; gap: 6px; padding: 12px 0; }
.panel-skeleton__curve { width: 100%; height: 70%; background: var(--color-surface-subtle); border-radius: var(--radius-sm); }
.panel-skeleton--rows { display: grid; gap: 8px; padding: 8px 0; }
.panel-skeleton__row { height: 14px; background: var(--color-surface-subtle); border-radius: 4px; }

/* ── Degraded ribbon (page-level, non-blocking) ────────────────── */
.overview-console__degraded-ribbon {
  display: flex; align-items: center; gap: 10px;
  padding: 8px 14px; border-radius: var(--radius-sm);
  background: var(--color-status-warning-soft);
  border: 1px solid color-mix(in srgb, var(--color-status-warning) 30%, transparent);
  color: var(--color-text); font-size: 13px;
}
.overview-console__degraded-ribbon-dot { width: 6px; height: 6px; border-radius: 50%; background: var(--color-status-warning); flex-shrink: 0; }
```

- [ ] **Step 3: Wire skeletons into the panels' loading branches**

In each overview panel, replace the `overview-panel--loading` spinner block with the matching `<PanelSkeleton variant=... />` inside the panel frame (keep the surrounding `<section className="overview-console__trend">` etc. so the frame is stable):
- `OverviewSummaryPanel.tsx` loading → `<PanelSkeleton variant="metrics" />`.
- `OverviewTrendPanel.tsx` loading → header + `<PanelSkeleton variant="chart" />`.
- `OverviewHeatmapPanel.tsx` loading → header + `<PanelSkeleton variant="chart" />`.
- `OverviewRankingsPanel.tsx` loading → `<PanelSkeleton variant="list" rows={5} />`.

Keep each panel's existing inline error text ("X unavailable") but render it inside the panel frame (it already is — confirm). The catastrophic summary-unavailable full-page `PageState` in `OverviewPage.tsx` stays (it's the one allowed full-page replacement).

- [ ] **Step 4: Replace the degraded banner with a thin ribbon**

In `apps/gui/src/pages/OverviewPage.tsx`, replace the `<PageState kind="degraded" .../>` block (the centered card) with:

```tsx
      {showDegraded && (
        <div className="overview-console__degraded-ribbon" role="status">
          <span className="overview-console__degraded-ribbon-dot" aria-hidden="true" />
          <span>{degradedReason ?? (summaryEnvelope?.is_stale ? "Showing stale data — refresh in progress" : "Data is approximate — exact aggregates not yet available")}</span>
        </div>
      )}
```

- [ ] **Step 5: Run overview tests + guard + build**

Run: `(cd apps/gui && npx vitest run src/components/overview/) && pnpm check:gui-surfaces && pnpm --filter @busytok/gui build`.
Expected: green (update panel tests' loading assertions if they checked for the old spinner text/loading class).

- [ ] **Step 6: Commit**

```bash
git add apps/gui/src/components/overview/PanelSkeleton.tsx apps/gui/src/components/overview/*.tsx apps/gui/src/pages/OverviewPage.tsx apps/gui/src/styles/pages.css
git commit -m "feat(gui): overview state-in-frame — skeletons + degraded ribbon

Panel loading states render a low-contrast in-frame skeleton (metrics/chart/
list) so the panel frame stays stable — no layout jump, no spinner card.
Page-level degraded becomes a thin ribbon (dot + line), not a centered card.
Catastrophic summary-unavailable stays the only full-page replacement. Phase 2."
```

---

### Task 7: Charts readout — LiveCurve

**Files:**
- Modify: `apps/gui/src/components/overview/LiveCurvePanel.tsx` (remove vertical grid; ≤8% fill; explicit chartTokens stroke)

**Interfaces:** none new; reuses `chartTokens`.

- [ ] **Step 1: Reduce the area fill to ≤8% + unify to chartTokens**

In `apps/gui/src/components/overview/LiveCurvePanel.tsx`, in `resolveLiveCurveThemeColors()` (currently lines 29–36), change `topColor` to a ≤8% alpha and source it from the live-primary token via a single resolve:

```ts
function resolveLiveCurveThemeColors() {
  // Chart stroke + fill both derive from the live-primary token; the fill is
  // kept ≤8% so the line reads as a system readout, not a marketing glow.
  const line = resolveCssColor("--color-data-live-primary", "#4f63f6");
  return {
    lineColor: line,
    topColor: resolveCssColor("--color-data-live-primary-soft", "rgba(79, 99, 246, 0.08)"),
    textColor: resolveCssColor("--color-text-muted", "#6e7480"),
    gridColor: resolveCssColor("--color-border-subtle", "rgba(17, 24, 39, 0.06)"),
  };
}
```

(If `--color-data-live-primary-soft` is currently 0.22, lower the token to 0.08 in `tokens.css` for both themes — that enforces ≤8% at the contract level rather than per-call. Update `tokens.test.ts` assertion for `--color-data-live-primary-soft` accordingly: light `rgba(79, 99, 246, 0.08)`, dark `rgba(167, 184, 255, 0.10)`.)

- [ ] **Step 2: Disable the vertical grid**

In the `createChart(...)` options (currently lines ~71–101), change the `grid` block so vertical lines are off:

```ts
    grid: {
      vertLines: { visible: false },
      horzLines: { color: themeColors.gridColor, count: 4 },
    },
```

(`count: 4` ≈ the "3–4 horizontal reference lines" requirement; `vertLines.visible: false` removes the vertical grid.)

- [ ] **Step 3: Run LiveCurve test + tokens test + build**

Run: `(cd apps/gui && npx vitest run src/components/overview/LiveCurvePanel.test.tsx src/styles/tokens.test.ts) && pnpm --filter @busytok/gui build`.
Expected: green (update `LiveCurvePanel.test.tsx` mock values if they assert the old 0.22 soft or grid shape — align mocks to the new values).

- [ ] **Step 4: Commit**

```bash
git add apps/gui/src/components/overview/LiveCurvePanel.tsx apps/gui/src/styles/tokens.css apps/gui/src/styles/tokens.test.ts apps/gui/src/components/overview/LiveCurvePanel.test.tsx
git commit -m "refactor(gui): live curve readout — no vertical grid, ≤8% fill

Vertical grid disabled; horizontal grid reduced to ~4 reference lines. Area
fill sourced from --color-data-live-primary-soft lowered to ≤8% at the token
layer (light/dark) so the curve reads as a system readout, not a glow. Phase 2."
```

---

### Task 8: Charts readout — trend grid + rankings accent + heatmap fixed legend

**Files:**
- Modify: `apps/gui/src/components/charts/NivoTimelineChart.tsx` (gridX off, gridY 4)
- Modify: `apps/gui/src/components/overview/OverviewRankingsPanel.tsx` + `apps/gui/src/styles/pages.css` (neutral bars + #1/selected accent)
- Modify: `apps/gui/src/components/overview/OverviewTokenHeatmap.tsx` (fixed 5-cell legend)

**Interfaces:** none new.

- [ ] **Step 1: Trend — drop vertical grid, 4 horizontal**

In `apps/gui/src/components/charts/NivoTimelineChart.tsx`, change:
- `gridXValues={axis.primaryGuideKeys}` → `gridXValues={[]}` (no vertical grid).
- `gridYValues={5}` → `gridYValues={4}`.

- [ ] **Step 2: Rankings — neutral bars + #1/selected indigo**

In `apps/gui/src/components/overview/OverviewRankingsPanel.tsx`, add an accent class to the first item of each section (and a `data-rank` for clarity):

```tsx
          {section.items.map((item, idx) => (
            <div key={item.id} className={`ranking-item${idx === 0 ? " ranking-item--leader" : ""}`}>
              <span className="ranking-item__bar" style={{ width: `${item.bar_value}%` }} />
              <span className="ranking-item__label">{item.label}</span>
              <span className="ranking-item__value">{item.value}</span>
            </div>
          ))}
```

In `apps/gui/src/styles/pages.css`, change `.ranking-item__bar` from `--color-data-primary` @ 0.07 to neutral by default, indigo for the leader:

```css
.ranking-item__bar { background: var(--color-data-neutral); opacity: 0.10; }   /* neutral default */
.ranking-item--leader .ranking-item__bar { background: var(--color-data-primary); opacity: 0.14; } /* one accent */
```

- [ ] **Step 3: Heatmap — fixed 5-cell legend**

In `apps/gui/src/components/overview/OverviewTokenHeatmap.tsx`, replace the dynamic `model.legendLevels.map(...)` legend block (currently lines ~205–213) with a fixed 5-cell legend (`[0, 1, 2, 3, 4]`), ignoring the sparse-mode collapse:

```tsx
              <div className="overview-heatmap__legend">
                <span className="overview-heatmap__legend-label">Less</span>
                {[0, 1, 2, 3, 4].map((level) => (
                  <span key={level} className={`overview-heatmap__legend-swatch overview-heatmap__cell--${level}`} />
                ))}
                <span className="overview-heatmap__legend-label">More</span>
              </div>
```

(Leave `lib/heatmap.ts`'s `legendLevels` computation in place — it still drives the intensity ramp for cells; only the legend rendering is fixed to 5. Add a code comment noting the legend is intentionally fixed per the design spec so sparse data doesn't shrink it.)

- [ ] **Step 4: Run chart/ranking/heatmap tests + guard + build**

Run: `(cd apps/gui && npx vitest run src/components/charts/ src/components/overview/) && pnpm check:gui-surfaces && pnpm --filter @busytok/gui build`.
Expected: green (update `OverviewPanels.test.tsx` if it asserts the dynamic legend length or ranking bar color — align to fixed-5 + leader class).

- [ ] **Step 5: Commit**

```bash
git add apps/gui/src/components/charts/NivoTimelineChart.tsx apps/gui/src/components/overview/OverviewRankingsPanel.tsx apps/gui/src/components/overview/OverviewTokenHeatmap.tsx apps/gui/src/styles/pages.css apps/gui/src/components/overview/OverviewPanels.test.tsx
git commit -m "refactor(gui): charts readout — trend grid, rankings accent, heatmap legend

Trend: vertical grid off, 4 horizontal reference lines. Rankings: neutral
bars with a single indigo accent on the #1/leader row. Heatmap: fixed 5-cell
legend (Less…More) that no longer collapses in sparse mode. Phase 2."
```

---

### Task 9: Prompt Palette command-surface (4 carriers)

**Files:**
- Modify: `apps/gui/src/components/prompt-palette/PromptPaletteOverlay.tsx` (accessory denoise: pin neutral; remove `prompt-overlay__pin` green)
- Modify: `apps/gui/src/pages/PromptPalettePage.tsx` (align row/selection/accessory grammar)
- Modify: `apps/gui/src/styles/components.css` (selected = neutral lift + rail; keycap/close no shadow; pin neutral)

**Interfaces:** none new — the 4 carriers keep their roles; this task aligns their visual grammar via CSS + small markup tweaks. (A fully shared row COMPONENT across overlay-listbox and page-table is out of scope — they have different interaction models; YAGNI. The spec's "share the same row grammar" is satisfied by consistent selected/hover/accessory treatment.)

- [ ] **Step 1: Overlay selected row → neutral lift + rail (no accent title)**

In `apps/gui/src/styles/components.css`, change `.prompt-overlay__row.is-selected`:

```css
.prompt-overlay__row.is-selected {
  background: var(--color-hover-strong);                 /* neutral lift, not accent tint */
  border-color: var(--color-border-subtle);
}
.prompt-overlay__row.is-selected::before { background: var(--color-accent-500); }  /* keep the left rail */
/* DELETE: .prompt-overlay__row.is-selected .prompt-overlay__title { color: var(--color-accent-600); } */
/* Title stays neutral high-contrast; the rail + bg-lift carry selection. */
```

- [ ] **Step 2: Pin → neutral (no green pill)**

In `apps/gui/src/styles/components.css`, change `.prompt-overlay__pin`:

```css
.prompt-overlay__pin {
  border-radius: var(--radius-sm);
  background: var(--color-surface-subtle);
  color: var(--color-text-faint);
  padding: 3px 8px; font-size: 10px; font-weight: 600;
  text-transform: uppercase; letter-spacing: 0.04em;
  /* was success-soft green — now neutral */
}
```

In `apps/gui/src/components/prompt-palette/PromptPaletteOverlay.tsx`, change the pin text from `Pinned` to a neutral `◇ Pinned` or keep `Pinned` — the class handles the neutral look. (No TS logic change.)

- [ ] **Step 3: Keycap + close — drop the card shadow**

In `apps/gui/src/styles/components.css`, in `.prompt-overlay__keycap` and `.prompt-overlay__close`, remove `box-shadow: var(--material-shadow-card);` (keep the 2px bottom-border key affordance on keycap). Confirm both use `border-radius: var(--radius-sm)` (6px).

- [ ] **Step 4: Page row — align selected/hover to the same grammar**

In `apps/gui/src/styles/pages.css` (or components.css, wherever `.prompt-row` lives), ensure the page's selected/hover states use the same tokens: hover `--color-hover`, selected `--color-hover-strong` + a 2px left accent rail (if the page has a selected state); accessory `.prompt-tag` stays neutral (already `--color-canvas-subtle`-ish → now `--color-surface-subtle` via Phase 1 rename). Confirm `.prompt-row__actions--recessed` stays (hover-revealed actions).

- [ ] **Step 5: Run palette tests + guard + build**

Run: `(cd apps/gui && npx vitest run src/components/prompt-palette/ src/pages/PromptPalettePage.test.tsx) && pnpm check:gui-surfaces && pnpm --filter @busytok/gui build`.
Expected: green (update palette tests if they assert the green pin class or accent-title selection).

- [ ] **Step 6: Commit**

```bash
git add apps/gui/src/components/prompt-palette/PromptPaletteOverlay.tsx apps/gui/src/pages/PromptPalettePage.tsx apps/gui/src/styles/components.css apps/gui/src/styles/pages.css apps/gui/src/components/prompt-palette/*.test.* apps/gui/src/pages/PromptPalettePage.test.tsx
git commit -m "refactor(gui): prompt palette command-surface across 4 carriers

Overlay selected row = neutral lift (--color-hover-strong) + left rail; title
stays neutral (no accent recolor). Pin tag neutral (no green pill). Keycap +
close drop the card shadow (keep the 2px key affordance). Page row aligned to
the same hover/selected/accessory grammar. Phase 2."
```

---

### Task 10: Hover migration + component-level dark verification

**Files:**
- Modify: `apps/gui/src/styles/components.css`, `pages.css` (migrate hover fills to `--color-hover` / `--color-hover-strong`)
- Modify: `apps/gui/src/styles/components.css` (dark-theme active-selector overrides where Phase 1's `accent-600` is too dim)

**Interfaces:** none new.

- [ ] **Step 1: Migrate hover fills to the hover token**

Search the two consumer CSS files for hover/active fills still using `--color-surface` / `--color-surface-subtle` / `--color-surface-elevated`(gone) as a hover background and replace with `--color-hover` (or `--color-hover-strong` for selected/active). Candidates (verify each in the file before editing): `.status-chip:hover`, `.desktop-icon-button:hover`, `.app-select__item[data-highlighted]`, `.segmented-control__option:hover`, `.prompt-overlay__row:hover` (already `--color-hover-strong`? confirm), `.ranking-item:hover`. Use:

```bash
rg -n ':hover|data-highlighted|is-active' apps/gui/src/styles/components.css apps/gui/src/styles/pages.css | rg 'background'
```

For each hit, replace the hover `background` value with `var(--color-hover)` (selected/active → `var(--color-hover-strong)`). Leave hover states that change `color` or `border-color` alone.

- [ ] **Step 2: Dark-theme accent-text overrides**

In `apps/gui/src/styles/components.css`, under `:root[data-theme="dark"]`, add overrides so dark selection/active text uses the bright accent tier (Phase 1 rule 11):

```css
:root[data-theme="dark"] .desktop-sidebar__item.is-active { color: var(--color-accent-400); }
:root[data-theme="dark"] .titlebar-chip.is-warning { color: var(--color-text); } /* keep legible */
```

(Add only where a dark-mode spot-check shows `accent-600` is too dim. Do not blanket-override.)

- [ ] **Step 3: Run full suite + guard + build**

Run: `(cd apps/gui && npx vitest run) && pnpm check:gui-surfaces && pnpm --filter @busytok/gui typecheck && pnpm --filter @busytok/gui build`.
Expected: 700+ tests PASS; guard + typecheck + build green.

- [ ] **Step 4: Commit**

```bash
git add apps/gui/src/styles/components.css apps/gui/src/styles/pages.css
git commit -m "refactor(gui): migrate hover→--color-hover; dark active accent-400

Hover/active fills across components now use --color-hover / --color-hover-strong
(no ad-hoc surface fills). Dark-theme active selection uses accent-400 (bright
tier) for legibility. Phase 2."
```

---

### Task 11: Phase 2 verification gate + checklist

**Files:**
- Verify only (no edits unless a gate check fails).

- [ ] **Step 1: Full verification gate**

Run (all must pass):

```bash
pnpm --filter @busytok/gui typecheck
cd apps/gui && npx vitest run          # full suite — expect all green, no hang
pnpm coverage:gui                       # lines ≥90% (the enforced threshold)
pnpm check:gui-surfaces                 # all guard rules green
pnpm --filter @busytok/gui build
```

Expected: typecheck clean; full suite PASS; coverage:gui PASS; guard exit 0; build succeeds.

- [ ] **Step 2: Coverage floor check for new Phase 2 TS**

Confirm the new modules are fully covered: `titlebarStatus.ts` (100%), `TitlebarStatusChip.tsx` (rendering paths), `PanelSkeleton.tsx` (all variants). If coverage on any new file is <90%, add the missing test cases (e.g. the danger-auxiliary popover detail branch, the `list`/`table` skeleton variants) before committing.

Run: `cd apps/gui && npx vitest run src/components/desktop/titlebarStatus.test.ts src/components/desktop/TitlebarStatusChip.test.tsx --coverage.enabled true --coverage.include 'src/components/desktop/titlebarStatus.ts'`

- [ ] **Step 3: Manual smoke (both themes)**

Run: `pnpm dev:gui`. Confirm in light + dark:
- Titlebar shows ONE calm chip when healthy (green dot, `Live capture active`); clicking opens the read-only popover; escalating readiness/connection/lag turns it amber in place.
- Sidebar: active item = left rail + accent text, no tint block; hover is a subtle lift.
- Overview: trend/live/heatmap are border-first (no heavy shadow); metrics are neutral (success not green); loading shows in-frame skeletons; degraded is a thin ribbon.
- Charts: no vertical grid; live fill is faint; rankings have one indigo leader bar; heatmap legend is 5 fixed cells.
- Prompt Palette (overlay + window + page): selected = neutral lift + rail; pin is neutral; keycaps have no shadow.

- [ ] **Step 4: Commit (gate-record commit if any test/assertion was touched)**

If Steps 1–3 required no edits, this is a no-commit verification. If any test/assertion was updated to match the new model, commit:

```bash
git commit -am "test(gui): phase 2 verification gate — full suite + coverage + guard green"
```

---

## Self-Review

**1. Spec coverage (Phase 2 = spec §6.1–6.6 + §7 component-level):**
- §6.1 Titlebar (single chip + read-only popover + escalation + telemetry) → Tasks 1–2. ✓
- §6.2 Sidebar (directory: rail + hover token, MONITORING rename, height 32, no branding) → Task 3. ✓
- §6.3 Metric cards (neutral default, delete `--success`, flag/dot, ratios) → Task 4. ✓
- §6.4 Overview 3 tiers (page shell section-gap + max-width 1600; Tier A/B/C; in-panel emphasis; state-in-frame + degraded ribbon) → Tasks 5–6. ✓
- §6.5 Charts (single indigo line, ≤8% fill, no vertical grid, faint axis; rankings neutral + one accent; heatmap fixed-5 legend) → Tasks 7–8. ✓
- §6.6 Prompt Palette (command-surface across 4 carriers; selected neutral-lift + rail; accessory denoise; keycap no shadow) → Task 9. ✓
- §5 hover→`--color-hover` migration + §7 component-level dark → Task 10. ✓
- Observability (`safeReportEvent` at escalation) → Task 2. ✓

**2. Placeholder scan:** every step has exact paths, code, commands, expected output. The two places that say "update the test if it asserts X" are explicit about WHICH assertion and WHY (the model changed) — not placeholders, they're test-maintenance instructions tied to a concrete old assertion.

**3. Type consistency:** `TitlebarStatus` / `TitlebarStatusInput` / `TitlebarTone` defined in Task 1 step 3, consumed verbatim in Task 2 (import + props). `PanelSkeleton` props `{ variant, rows }` defined Task 6 step 1, consumed in step 3 with the same variant literals. `--color-hover` / `--color-hover-strong` defined in Phase 1, consumed in Tasks 3/9/10. `chartTokens.linePrimary` (Phase 1, post-`borderSoft` rename) consumed in Task 8.

**4. Coverage discipline:** every new TS module (`titlebarStatus.ts`, `TitlebarStatusChip.tsx`, `PanelSkeleton.tsx`) ships with tests; Task 11 step 2 asserts ≥90% on the new files. CSS-only tasks don't lower TS coverage.

**5. Out of scope (explicit):** shared Prompt-Palette row *component* (overlay-listbox vs page-table have different interaction models — YAGNI); new backend/protocol fields (last-event time, last-sync) — popover shows available fields only per the spec's data-boundary rule; a `--color-chart-grid` token (reuses `--color-border-subtle`).

**6. Review-driven hardening (applied before execution):**
- **`+1 danger` is gated by an explicit ID allowlist** (`BLOCKING_DANGER_CHIP_IDS = {"scan"}`, the only danger emitter per `supervisor.rs:1397` — `scan_state == "offline"`), not bare `tone === "danger"`. Non-allowlisted danger chips stay a single warning (spec: only blocking issues get `+1`).
- **`syncAggregateLagTelemetry` is mandated to stay (dual-track):** it emits the threshold/recovered events the new UI-level `gui.titlebar.status_escalated` cannot replace. The new event is additive.
- **Action routing uses a shared `statusActionToPage`** (extracted from `StatusChip`); unknown actions → `undefined` → no button rendered, no navigation, **no `overview` fallback**.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-23-busytok-geist-refactor-phase2-components.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
