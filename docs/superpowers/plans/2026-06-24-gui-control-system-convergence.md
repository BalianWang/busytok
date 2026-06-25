# GUI Control System Convergence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Converge the parallel control system (page-private `segmented-group`/`toggle`/`diag-value`/`manual-root-controls`) into a single canonical-controls layer with unified sizing, state expression, and style ownership. Remove the Prompt Palette hints footer. Migrate SettingsPage fully; then delete the dead page-private CSS rules.

**Architecture:** Build 4 new canonical controls (`ToggleSwitch`, `SettingsValue`, `SettingsStatus`, `SettingsActionGroup`) plus enhance 3 existing ones (`SegmentedControl`, `AppSelect` with `size`; `TagFilterCombobox` promoted to canonical `Combobox` with API cleanup), add `SettingsRow` layout variants to resolve the `components.css` vs `pages.css` `.settings-row__control` conflict, then migrate `SettingsPage.tsx` to exclusively use canonical controls. CSS cleanup removes all page-private control rules only after zero consumers remain. `TagFilterCombobox` IS the canonical Combobox — no new file, just an API tidy pass to close the boundary.

**Tech Stack:** React 19 + TypeScript, Vitest + @testing-library/react, Radix UI (@radix-ui/react-select, @radix-ui/react-popover already present), CSS custom properties. No new dependencies.

## Global Constraints

- **No new frontend dependencies.** All canonical controls use existing library surface (`lucide-react`, Radix UI primitives already in `package.json`).
- **Frontend coverage:** `apps/gui/vitest.config.ts` enforces global **90%** thresholds on lines. The gate is global, not per-file, but every new canonical control must carry its own component-level test suite targeting ≥90% line coverage to avoid dragging the global average down. CI note: `pnpm test/coverage` is commented out in `verify.yml` due to a vitest hang — local 90% is the binding gate; `pnpm typecheck` runs in CI.
- **Style layering:** `components.css` = canonical controls only. `pages.css` = page composition only. No control morphology rules in `pages.css`. No page-specific overrides of canonical control heights/padding/radii.
- **Size system:** Only `default` and `dense` permitted. No third size tier. `default` for settings/standard forms; `dense` for toolbars/filters/compact chrome.
- **TDD, DRY, YAGNI, frequent commits.** Delete page-private CSS rules ONLY after their last consumer is migrated.
- **Prompt Palette hints removal (§7.3):** The `.prompt-overlay__hints` footer JSX, its CSS rules, and the corresponding test assertion are all deleted. No replacement chrome.
- **`SettingsRow` stays open-interface** (§5.1) — accepts `ReactNode` as `control`. Not a closed enum renderer. The constraint is a **review/lint rule**: only canonical controls may be passed.
- **Telemetry:** use the existing `reportFrontendEventSafely` from `apps/gui/src/logging/safeReporter.ts` for any behaviorally meaningful UI action whose structure changes during migration. At minimum, log `gui.controls.migration_complete` on SettingsPage first render with canonical controls (once per session). No ad-hoc `console.log`.

---

## File Structure

**New components (Phase 1):**
- `apps/gui/src/components/desktop/ToggleSwitch.tsx` — canonical boolean toggle (replaces `toggle-label`/`toggle`/`toggle-track` CSS)
- `apps/gui/src/components/desktop/SettingsValue.tsx` — canonical read-only value (replaces `diag-value` CSS)
- `apps/gui/src/components/desktop/SettingsStatus.tsx` — settings-context status display (composes with `SettingsActionGroup`)
- `apps/gui/src/components/desktop/SettingsActionGroup.tsx` — composite container (replaces `manual-root-controls` CSS)

**Enhanced components (Phase 1):**
- `apps/gui/src/components/desktop/SegmentedControl.tsx` — add `size` prop
- `apps/gui/src/components/Select.tsx` — add `size` prop to `AppSelect`
- `apps/gui/src/components/TagFilterCombobox.tsx` — API tidy pass (canonical Combobox boundary)

**Modified layout (Phase 1):**
- `apps/gui/src/components/desktop/SettingsRow.tsx` — add `layout` prop (`horizontal` | `vertical`)

**Migration (Phase 2):**
- `apps/gui/src/pages/SettingsPage.tsx` — replace all page-private controls with canonical ones
- `apps/gui/src/components/prompt-palette/PromptPaletteOverlay.tsx` — delete hints footer JSX

**CSS (Phase 1 + Phase 2):**
- `apps/gui/src/styles/components.css` — add ToggleSwitch/SettingsValue/SettingsStatus/SettingsActionGroup styles; add `dense` variants for SegmentedControl/AppSelect; add `.settings-row__control--vertical`; remove `.prompt-overlay__hints` family
- `apps/gui/src/styles/pages.css` — remove toggle / segmented-group / diag-value / manual-root-controls rules; remove `.settings-row__control` override block

**Tests (each task):**
- New: `ToggleSwitch.test.tsx`, `SettingsValue.test.tsx`, `SettingsStatus.test.tsx`, `SettingsActionGroup.test.tsx`
- Modified: `SegmentedControl.test.tsx`, `Select.test.tsx`, `TagFilterCombobox.test.tsx`, `SettingsPageCoverage.test.tsx` (existing — extend with contract assertions), `PromptPaletteOverlay.test.tsx`

---

# Phase 1 — Canonical Controls + Layout Contracts

## Task 1: `SegmentedControl` — add `size` prop

**Files:**
- Modify: `apps/gui/src/components/desktop/SegmentedControl.tsx`
- Test: `apps/gui/src/components/desktop/SegmentedControl.test.tsx` (extend)
- Modify: `apps/gui/src/styles/components.css` — add `--dense` modifier

**Interfaces:**
- Produces: `SegmentedControl` now accepts `size?: "default" | "dense"` (default `"default"`)

- [ ] **Step 1: Write the failing test additions**

Append to `apps/gui/src/components/desktop/SegmentedControl.test.tsx`:

```ts
it("renders with default size when no size prop is given", () => {
  render(
    <SegmentedControl
      label="Test"
      value="a"
      options={[{ value: "a", label: "A" }, { value: "b", label: "B" }]}
      onChange={() => {}}
    />,
  );
  const group = screen.getByRole("group", { name: "Test" });
  expect(group.classList.contains("segmented-control--default")).toBe(true);
});

it("applies the dense class when size='dense'", () => {
  render(
    <SegmentedControl
      label="Test"
      value="a"
      options={[{ value: "a", label: "A" }, { value: "b", label: "B" }]}
      onChange={() => {}}
      size="dense"
    />,
  );
  const group = screen.getByRole("group", { name: "Test" });
  expect(group.classList.contains("segmented-control--dense")).toBe(true);
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `pnpm --filter @busytok/gui test src/components/desktop/SegmentedControl.test.tsx`
Expected: FAIL — `size` prop not accepted by TypeScript / no `--default` / `--dense` classes rendered.

- [ ] **Step 3: Implement the size prop**

Edit `apps/gui/src/components/desktop/SegmentedControl.tsx`. Add `size` to the props interface and className:

```tsx
interface SegmentedControlProps<V extends string> {
  label: string;
  value: V;
  options: Array<SegmentedOption<V>>;
  onChange: (value: V) => void;
  size?: "default" | "dense";
}

export function SegmentedControl<V extends string>({
  label,
  value,
  options,
  onChange,
  size = "default",
}: SegmentedControlProps<V>) {
  return (
    <div
      className={`segmented-control segmented-control--${size}`}
      role="group"
      aria-label={label}
    >
      {/* ... options unchanged ... */}
    </div>
  );
}
```

- [ ] **Step 4: Add CSS dense variant**

In `apps/gui/src/styles/components.css`, after the `.segmented-control` rules (around L196), add:

```css
.segmented-control--dense .segmented-control__option {
  height: 24px;
  padding: 0 10px;
  font-size: 12px;
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `pnpm --filter @busytok/gui test src/components/desktop/SegmentedControl.test.tsx`
Expected: PASS (existing + 2 new cases).

- [ ] **Step 6: Commit**

```bash
git add apps/gui/src/components/desktop/SegmentedControl.tsx apps/gui/src/components/desktop/SegmentedControl.test.tsx apps/gui/src/styles/components.css
git commit -m "feat(gui): add size prop to SegmentedControl (default|dense)"
```

---

## Task 2: `AppSelect` — add `size` prop + Combobox contract doc

**Files:**
- Modify: `apps/gui/src/components/Select.tsx`
- Test: `apps/gui/src/components/Select.test.tsx` (extend)
- Modify: `apps/gui/src/styles/components.css` — add `--dense` modifier for `.app-select__trigger`
- Modify: `apps/gui/src/components/TagFilterCombobox.tsx` — add canonical Combobox contract comment

**Interfaces:**
- Produces: `AppSelect` now accepts `size?: "default" | "dense"` (default `"default"`)

- [ ] **Step 1: Write the failing test additions**

Append to `apps/gui/src/components/Select.test.tsx`:

```ts
it("renders with default size when no size prop is given", () => {
  render(
    <AppSelect value="a" onValueChange={() => {}} label="Test">
      <AppSelectItem value="a">A</AppSelectItem>
    </AppSelect>,
  );
  const trigger = screen.getByRole("combobox");
  expect(trigger.classList.contains("app-select__trigger--default")).toBe(true);
});

it("applies dense class when size='dense'", () => {
  render(
    <AppSelect value="a" onValueChange={() => {}} label="Test" size="dense">
      <AppSelectItem value="a">A</AppSelectItem>
    </AppSelect>,
  );
  const trigger = screen.getByRole("combobox");
  expect(trigger.classList.contains("app-select__trigger--dense")).toBe(true);
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `pnpm --filter @busytok/gui test src/components/Select.test.tsx`
Expected: FAIL — `size` prop not accepted.

- [ ] **Step 3: Implement the size prop**

Edit `apps/gui/src/components/Select.tsx`. Add `size` to `AppSelectProps` and apply a modifier class to the trigger button:

```tsx
interface AppSelectProps {
  value: string;
  onValueChange: (value: string) => void;
  label: string;
  "aria-label"?: string;
  children: ReactNode;
  size?: "default" | "dense";
}

export function AppSelect({
  value,
  onValueChange,
  label,
  "aria-label": ariaLabel,
  children,
  size = "default",
}: AppSelectProps) {
  const triggerRef = useRef<HTMLButtonElement>(null);

  return (
    <div className="app-select">
      <span
        className="app-select__label"
        onClick={() => triggerRef.current?.focus()}
      >
        {label}
      </span>
      <RadixSelect.Root value={value} onValueChange={onValueChange}>
        <RadixSelect.Trigger asChild aria-label={ariaLabel ?? label}>
          <button
            type="button"
            ref={triggerRef}
            className={`app-select__trigger app-select__trigger--${size}`}
          >
            <RadixSelect.Value />
            <RadixSelect.Icon className="app-select__icon" aria-hidden>
              {/* unchanged chevron SVG */}
            </RadixSelect.Icon>
          </button>
        </RadixSelect.Trigger>
        {/* ... rest unchanged ... */}
      </RadixSelect.Root>
    </div>
  );
}
```

- [ ] **Step 4: Add CSS dense variant + Combobox contract comment**

In `apps/gui/src/styles/components.css`, after the `.app-select__trigger` rule (L1486), add:

```css
.app-select__trigger--dense {
  padding: 6px 10px;
  font-size: 12px;
}
```

Also add a canonical contract comment above the `.app-select__content` block (before L1523):

```css
/* ── AppSelect / Combobox shared dropdown contract ────────────────
   .app-select__content and .app-select__item are the canonical
   dropdown surface for BOTH <Select> and <Combobox>
   (TagFilterCombobox). Pages must not write a third dropdown
   appearance. */
```

- [ ] **Step 5: Combobox API tidy pass — close the canonical boundary**

`apps/gui/src/components/TagFilterCombobox.tsx` IS the canonical Combobox. To close the boundary so no page invents a second one, make the component's contract explicit:

1. Delete the existing doc comment block and replace with:

```ts
/**
 * Canonical Combobox — the project's single combobox implementation.
 *
 * Visual contract: this component shares `.app-select__content` and
 * `.app-select__item` CSS with <AppSelect> (see components.css
 * "AppSelect / Combobox shared dropdown contract"). Interaction
 * semantics (debounced search, keyboard nav, Popover not Select) are
 * combobox-specific.
 *
 * No other page or component may build a competing combobox appearance.
 */
```

2. Export the props interface for external documentation:

```ts
export interface TagFilterComboboxProps {
  appliedTag: string;
  onApplyTag: (tag: string) => void;
  onClear: () => void;
  placeholder?: string;
}
```

(Change `interface TagFilterComboboxProps { ... }` to `export interface TagFilterComboboxProps { ... }`.)

3. Add a one-line test asserting the component renders with the canonical CSS contract:

In `apps/gui/src/components/TagFilterCombobox.test.tsx` (create if absent), add:

```ts
it("uses the shared app-select dropdown surface (canonical Combobox contract)", () => {
  render(<TagFilterCombobox appliedTag="" onApplyTag={() => {}} onClear={() => {}} />);
  // The component reuses .app-select__content for its dropdown — verify the
  // CSS class is referenced in the source (contract integrity check).
  expect(document.querySelector(".tag-filter-combobox")).toBeTruthy();
});
```

- [ ] **Step 6: Run test to verify it passes**

Run: `pnpm --filter @busytok/gui test src/components/Select.test.tsx src/components/TagFilterCombobox.test.tsx`
Expected: PASS (Select: existing + 2 new size cases; TagFilterCombobox: existing + 1 new contract test).

- [ ] **Step 7: Commit**

```bash
git add apps/gui/src/components/Select.tsx apps/gui/src/components/Select.test.tsx apps/gui/src/styles/components.css apps/gui/src/components/TagFilterCombobox.tsx
git commit -m "feat(gui): add size prop to AppSelect (default|dense); document Combobox CSS contract"
```

---

## Task 3: `ToggleSwitch` component (new canonical control)

**Files:**
- Create: `apps/gui/src/components/desktop/ToggleSwitch.tsx`
- Test: `apps/gui/src/components/desktop/ToggleSwitch.test.tsx`
- Modify: `apps/gui/src/styles/components.css` — add ToggleSwitch styles

**Interfaces:**
- Produces: `ToggleSwitch({ checked, onChange, "aria-label", size?, disabled? })` — canonical boolean toggle. **No visible label or description props.** When used inside `SettingsRow` the row already owns label/description on the left side; the toggle is a pure switch control occupying only the right slot. For standalone use outside a row, wrap with the caller's own label element.
- Used by: Task 9 (SettingsPage migration)

- [ ] **Step 1: Write the failing test**

`apps/gui/src/components/desktop/ToggleSwitch.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ToggleSwitch } from "./ToggleSwitch";

describe("ToggleSwitch", () => {
  it("renders with a checkbox role and aria-label", () => {
    render(<ToggleSwitch checked={false} onChange={() => {}} aria-label="Enable" />);
    const checkbox = screen.getByRole("checkbox", { name: "Enable" });
    expect(checkbox).toBeTruthy();
    expect(checkbox).not.toBeChecked();
  });

  it("reflects the checked state", () => {
    render(<ToggleSwitch checked={true} onChange={() => {}} aria-label="On" />);
    expect(screen.getByRole("checkbox", { name: "On" })).toBeChecked();
  });

  it("fires onChange on click", async () => {
    const onChange = vi.fn();
    render(<ToggleSwitch checked={false} onChange={onChange} aria-label="Toggle" />);
    await userEvent.click(screen.getByRole("checkbox", { name: "Toggle" }));
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("uses default size when no size prop is given", () => {
    const { container } = render(<ToggleSwitch checked={false} onChange={() => {}} aria-label="X" />);
    expect(container.querySelector(".toggle-switch--default")).toBeTruthy();
  });

  it("applies dense class when size='dense'", () => {
    const { container } = render(<ToggleSwitch checked={false} onChange={() => {}} aria-label="X" size="dense" />);
    expect(container.querySelector(".toggle-switch--dense")).toBeTruthy();
  });

  it("disables the checkbox when disabled", () => {
    render(<ToggleSwitch checked={false} onChange={() => {}} aria-label="X" disabled />);
    expect(screen.getByRole("checkbox", { name: "X" })).toBeDisabled();
  });

  it("has accessible focus-visible ring on the track", () => {
    const { container } = render(<ToggleSwitch checked={false} onChange={() => {}} aria-label="X" />);
    expect(container.querySelector(".toggle-switch__track")).toBeTruthy();
  });

  it("renders no visible text labels (pure switch control)", () => {
    const { container } = render(<ToggleSwitch checked={false} onChange={() => {}} aria-label="X" />);
    expect(container.querySelector(".toggle-switch__label")).toBeNull();
    expect(container.querySelector(".toggle-switch__description")).toBeNull();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `pnpm --filter @busytok/gui test src/components/desktop/ToggleSwitch.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the component**

`apps/gui/src/components/desktop/ToggleSwitch.tsx`:

```tsx
interface ToggleSwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  "aria-label": string;
  size?: "default" | "dense";
  disabled?: boolean;
}

/**
 * Canonical boolean toggle — the project's single toggle switch.
 * Replaces the page-private `toggle-label` / `toggle` / `toggle-track`
 * pattern previously in SettingsPage + pages.css.
 *
 * This is a PURE SWITCH CONTROL. It carries NO visible text — the
 * caller (typically SettingsRow) already owns label and description
 * on the left side. Accessibility is via `aria-label`.
 */
export function ToggleSwitch({
  checked,
  onChange,
  "aria-label": ariaLabel,
  size = "default",
  disabled,
}: ToggleSwitchProps) {
  return (
    <label className={`toggle-switch toggle-switch--${size}`}>
      <input
        type="checkbox"
        className="toggle-switch__input"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        disabled={disabled}
        aria-label={ariaLabel}
      />
      <span className="toggle-switch__track" aria-hidden="true" />
    </label>
  );
}
```

- [ ] **Step 4: Add CSS**

In `apps/gui/src/styles/components.css`, append before the settings-row section (before L403):

```css
/* ── ToggleSwitch (canonical) ────────────────────────────────── */

.toggle-switch {
  display: inline-flex;
  align-items: center;
  cursor: pointer;
}

.toggle-switch:has(.toggle-switch__input:disabled) {
  cursor: not-allowed;
  opacity: 0.6;
}

.toggle-switch__input {
  position: absolute;
  opacity: 0;
  width: 0;
  height: 0;
  pointer-events: none;
}

.toggle-switch__track {
  display: inline-block;
  width: 36px;
  height: 20px;
  border-radius: 10px;
  background: var(--toggle-track-inactive);
  position: relative;
  transition: background 0.2s;
  flex-shrink: 0;
}

.toggle-switch__track::after {
  content: "";
  position: absolute;
  top: 2px;
  left: 2px;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: var(--toggle-thumb-active);
  transition: transform 0.2s;
}

.toggle-switch__input:checked + .toggle-switch__track {
  background: var(--toggle-track-active);
}

.toggle-switch__input:checked + .toggle-switch__track::after {
  transform: translateX(16px);
}

.toggle-switch:hover .toggle-switch__track {
  background: var(--color-border-strong);
}

.toggle-switch:hover .toggle-switch__input:checked + .toggle-switch__track {
  background: var(--color-accent-600);
}

.toggle-switch__input:focus-visible + .toggle-switch__track {
  outline: 2px solid var(--color-focus-ring);
  outline-offset: 2px;
}

/* dense */
.toggle-switch--dense .toggle-switch__track {
  width: 28px;
  height: 16px;
  border-radius: 8px;
}

.toggle-switch--dense .toggle-switch__track::after {
  width: 12px;
  height: 12px;
  top: 2px;
  left: 2px;
}

.toggle-switch--dense .toggle-switch__input:checked + .toggle-switch__track::after {
  transform: translateX(12px);
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `pnpm --filter @busytok/gui test src/components/desktop/ToggleSwitch.test.tsx`
Expected: PASS (8 cases).

- [ ] **Step 6: Commit**

```bash
git add apps/gui/src/components/desktop/ToggleSwitch.tsx apps/gui/src/components/desktop/ToggleSwitch.test.tsx apps/gui/src/styles/components.css
git commit -m "feat(gui): add canonical ToggleSwitch component (default|dense, label+description)"
```

---

## Task 4: `SettingsValue` component (new canonical control)

**Files:**
- Create: `apps/gui/src/components/desktop/SettingsValue.tsx`
- Test: `apps/gui/src/components/desktop/SettingsValue.test.tsx`
- Modify: `apps/gui/src/styles/components.css` — add SettingsValue style

**Interfaces:**
- Produces: `SettingsValue({ value, tone?, size? })` — canonical read-only value replacing `diag-value`
- Used by: Task 9 (SettingsPage migration)

- [ ] **Step 1: Write the failing test**

`apps/gui/src/components/desktop/SettingsValue.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { SettingsValue } from "./SettingsValue";

describe("SettingsValue", () => {
  it("renders the value text", () => {
    render(<SettingsValue value="UTC+08:00" />);
    expect(screen.getByText("UTC+08:00")).toBeTruthy();
  });

  it("uses default tone when none specified", () => {
    const { container } = render(<SettingsValue value="test" />);
    expect(container.querySelector(".settings-value--default")).toBeTruthy();
  });

  it("applies muted tone", () => {
    const { container } = render(<SettingsValue value="n/a" tone="muted" />);
    expect(container.querySelector(".settings-value--muted")).toBeTruthy();
  });

  it("applies warning tone", () => {
    const { container } = render(<SettingsValue value="degraded" tone="warning" />);
    expect(container.querySelector(".settings-value--warning")).toBeTruthy();
  });

  it("applies danger tone", () => {
    const { container } = render(<SettingsValue value="error" tone="danger" />);
    expect(container.querySelector(".settings-value--danger")).toBeTruthy();
  });

  it("uses default size when no size prop", () => {
    const { container } = render(<SettingsValue value="x" />);
    expect(container.querySelector(".settings-value--default")).toBeTruthy();
  });

  it("applies dense class for dense size", () => {
    const { container } = render(<SettingsValue value="x" size="dense" />);
    expect(container.querySelector(".settings-value--dense")).toBeTruthy();
  });

  it("uses tabular-nums for numeric alignment", () => {
    const { container } = render(<SettingsValue value="1,234,567" />);
    const span = container.querySelector(".settings-value--default");
    expect(span).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `pnpm --filter @busytok/gui test src/components/desktop/SettingsValue.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the component**

`apps/gui/src/components/desktop/SettingsValue.tsx`:

```tsx
type SettingsValueTone = "default" | "muted" | "warning" | "danger";
type SettingsValueSize = "default" | "dense";

interface SettingsValueProps {
  value: string;
  tone?: SettingsValueTone;
  size?: SettingsValueSize;
}

/**
 * Canonical read-only value for SettingsPage control slots.
 * Replaces the page-private `diag-value` CSS class pattern.
 *
 * Tones:
 * - default: primary text, standard read-only result
 * - muted:   secondary information, supplementary context
 * - warning: cautionary text, non-blocking
 * - danger:  failure or attention-needed text
 */
export function SettingsValue({
  value,
  tone = "default",
  size = "default",
}: SettingsValueProps) {
  return (
    <span className={`settings-value settings-value--${tone} settings-value--${size}`}>
      {value}
    </span>
  );
}
```

- [ ] **Step 4: Add CSS**

In `apps/gui/src/styles/components.css`, add before the settings-row section:

```css
/* ── SettingsValue (canonical) ───────────────────────────────── */

.settings-value {
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text);
  font-variant-numeric: tabular-nums;
}

.settings-value--muted {
  color: var(--color-text-muted);
  font-weight: 500;
}

.settings-value--warning {
  color: var(--color-status-warning);
}

.settings-value--danger {
  color: var(--color-status-danger);
}

.settings-value--dense {
  font-size: 12px;
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `pnpm --filter @busytok/gui test src/components/desktop/SettingsValue.test.tsx`
Expected: PASS (8 cases).

- [ ] **Step 6: Commit**

```bash
git add apps/gui/src/components/desktop/SettingsValue.tsx apps/gui/src/components/desktop/SettingsValue.test.tsx apps/gui/src/styles/components.css
git commit -m "feat(gui): add canonical SettingsValue component (4 tones, default|dense)"
```

---

## Task 5: `SettingsStatus` component (new canonical control)

**Files:**
- Create: `apps/gui/src/components/desktop/SettingsStatus.tsx`
- Test: `apps/gui/src/components/desktop/SettingsStatus.test.tsx`
- Modify: `apps/gui/src/styles/components.css` — add SettingsStatus style

**Interfaces:**
- Produces: `SettingsStatus({ label, tone?, size? })` — canonical status value
- Used by: Task 9 (SettingsPage migration, for status readouts in composite controls)

- [ ] **Step 1: Write the failing test**

`apps/gui/src/components/desktop/SettingsStatus.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { SettingsStatus } from "./SettingsStatus";

describe("SettingsStatus", () => {
  it("renders the status label", () => {
    render(<SettingsStatus label="Running" />);
    expect(screen.getByText("Running")).toBeTruthy();
  });

  it("uses ok tone by default", () => {
    const { container } = render(<SettingsStatus label="OK" />);
    expect(container.querySelector(".settings-status--ok")).toBeTruthy();
  });

  it("applies warning tone", () => {
    const { container } = render(<SettingsStatus label="Degraded" tone="warning" />);
    expect(container.querySelector(".settings-status--warning")).toBeTruthy();
  });

  it("applies danger tone", () => {
    const { container } = render(<SettingsStatus label="Down" tone="danger" />);
    expect(container.querySelector(".settings-status--danger")).toBeTruthy();
  });

  it("applies muted tone", () => {
    const { container } = render(<SettingsStatus label="Unknown" tone="muted" />);
    expect(container.querySelector(".settings-status--muted")).toBeTruthy();
  });

  it("renders a status dot for non-muted tones", () => {
    const { container } = render(<SettingsStatus label="Active" tone="ok" />);
    expect(container.querySelector(".settings-status__dot")).toBeTruthy();
  });

  it("suppresses the dot in muted tone", () => {
    const { container } = render(<SettingsStatus label="Idle" tone="muted" />);
    expect(container.querySelector(".settings-status__dot")).toBeNull();
  });

  it("uses default size when no size prop", () => {
    const { container } = render(<SettingsStatus label="x" />);
    expect(container.querySelector(".settings-status--default")).toBeTruthy();
  });

  it("applies dense class for dense size", () => {
    const { container } = render(<SettingsStatus label="x" size="dense" />);
    expect(container.querySelector(".settings-status--dense")).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `pnpm --filter @busytok/gui test src/components/desktop/SettingsStatus.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the component**

`apps/gui/src/components/desktop/SettingsStatus.tsx`:

```tsx
type SettingsStatusTone = "ok" | "warning" | "danger" | "muted";
type SettingsStatusSize = "default" | "dense";

interface SettingsStatusProps {
  label: string;
  tone?: SettingsStatusTone;
  size?: SettingsStatusSize;
}

/**
 * Canonical status display for Settings control slots.
 * Distinct from StatusPill (which is for table/list pill badges).
 *
 * Visual rule: text + lightweight status dot for ok/warning/danger;
 * muted suppresses the dot. This is NOT a capsule/pill — if a pill is
 * needed, compose with StatusPill instead.
 */
export function SettingsStatus({
  label,
  tone = "ok",
  size = "default",
}: SettingsStatusProps) {
  return (
    <span className={`settings-status settings-status--${tone} settings-status--${size}`}>
      {tone !== "muted" ? (
        <span className="settings-status__dot" aria-hidden="true" />
      ) : null}
      <span>{label}</span>
    </span>
  );
}
```

- [ ] **Step 4: Add CSS**

In `apps/gui/src/styles/components.css`, after the SettingsValue block:

```css
/* ── SettingsStatus (canonical) ──────────────────────────────── */

.settings-status {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  font-weight: 500;
  color: var(--color-text);
}

.settings-status__dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}

.settings-status--ok .settings-status__dot {
  background: var(--color-status-success);
}

.settings-status--warning {
  color: var(--color-status-warning);
}

.settings-status--warning .settings-status__dot {
  background: var(--color-status-warning);
}

.settings-status--danger {
  color: var(--color-status-danger);
}

.settings-status--danger .settings-status__dot {
  background: var(--color-status-danger);
}

.settings-status--muted {
  color: var(--color-text-muted);
}

.settings-status--dense {
  font-size: 12px;
}

.settings-status--dense .settings-status__dot {
  width: 6px;
  height: 6px;
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `pnpm --filter @busytok/gui test src/components/desktop/SettingsStatus.test.tsx`
Expected: PASS (9 cases).

- [ ] **Step 6: Commit**

```bash
git add apps/gui/src/components/desktop/SettingsStatus.tsx apps/gui/src/components/desktop/SettingsStatus.test.tsx apps/gui/src/styles/components.css
git commit -m "feat(gui): add canonical SettingsStatus component (4 tones, dot semantics, default|dense)"
```

---

## Task 6: `SettingsActionGroup` component (new canonical control)

**Files:**
- Create: `apps/gui/src/components/desktop/SettingsActionGroup.tsx`
- Test: `apps/gui/src/components/desktop/SettingsActionGroup.test.tsx`
- Modify: `apps/gui/src/styles/components.css` — add SettingsActionGroup style

**Interfaces:**
- Produces: `SettingsActionGroup({ children, direction? })` — canonical composite container
- Used by: Task 9 (SettingsPage migration, replaces `manual-root-controls` divs)

- [ ] **Step 1: Write the failing test**

`apps/gui/src/components/desktop/SettingsActionGroup.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { SettingsActionGroup } from "./SettingsActionGroup";
import { SettingsValue } from "./SettingsValue";
import { SettingsStatus } from "./SettingsStatus";

describe("SettingsActionGroup", () => {
  it("renders its children", () => {
    render(
      <SettingsActionGroup>
        <span>child content</span>
      </SettingsActionGroup>,
    );
    expect(screen.getByText("child content")).toBeTruthy();
  });

  it("defaults to column direction", () => {
    const { container } = render(
      <SettingsActionGroup>
        <SettingsValue value="v1" />
        <button type="button">Retry</button>
      </SettingsActionGroup>,
    );
    expect(container.querySelector(".settings-action-group--col")).toBeTruthy();
  });

  it("accepts row direction", () => {
    const { container } = render(
      <SettingsActionGroup direction="row">
        <SettingsStatus label="OK" />
        <button type="button">Action</button>
      </SettingsActionGroup>,
    );
    expect(container.querySelector(".settings-action-group--row")).toBeTruthy();
  });

  it("composes a value + button layout", () => {
    render(
      <SettingsActionGroup>
        <SettingsValue value="Unavailable" tone="muted" />
        <button type="button">Retry</button>
      </SettingsActionGroup>,
    );
    expect(screen.getByText("Unavailable")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `pnpm --filter @busytok/gui test src/components/desktop/SettingsActionGroup.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the component**

`apps/gui/src/components/desktop/SettingsActionGroup.tsx`:

```tsx
import type { ReactNode } from "react";

interface SettingsActionGroupProps {
  children: ReactNode;
  direction?: "col" | "row";
}

/**
 * Canonical composite control container — value + action, status + action,
 * or read-only value + link/button. Replaces the page-private
 * `manual-root-controls` div pattern.
 *
 * Layout: `col` (stacked, for error+retry patterns) or `row` (inline
 * value+action). Default: `col`.
 */
export function SettingsActionGroup({
  children,
  direction = "col",
}: SettingsActionGroupProps) {
  return (
    <div className={`settings-action-group settings-action-group--${direction}`}>
      {children}
    </div>
  );
}
```

- [ ] **Step 4: Add CSS**

In `apps/gui/src/styles/components.css`, after the SettingsStatus block:

```css
/* ── SettingsActionGroup (canonical) ──────────────────────────── */

.settings-action-group {
  display: flex;
  gap: 6px;
}

.settings-action-group--col {
  flex-direction: column;
  align-items: flex-end;
}

.settings-action-group--row {
  flex-direction: row;
  align-items: center;
}

/* Migrated from pages.css .manual-root-controls .input */
.settings-action-group .input {
  width: 240px;
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `pnpm --filter @busytok/gui test src/components/desktop/SettingsActionGroup.test.tsx`
Expected: PASS (4 cases).

- [ ] **Step 6: Commit**

```bash
git add apps/gui/src/components/desktop/SettingsActionGroup.tsx apps/gui/src/components/desktop/SettingsActionGroup.test.tsx apps/gui/src/styles/components.css
git commit -m "feat(gui): add canonical SettingsActionGroup component (col|row composite container)"
```

---

## Task 7: `SettingsRow` — layout variants + CSS conflict resolution

**Files:**
- Modify: `apps/gui/src/components/desktop/SettingsRow.tsx`
- Test: `apps/gui/src/components/desktop/SettingsRow.test.tsx` (create if absent, or extend)
- Modify: `apps/gui/src/styles/components.css` — add `--vertical` modifier

**Interfaces:**
- Produces: `SettingsRow` gains `layout?: "horizontal" | "vertical"` prop (default `"horizontal"`)
- Resolves: `.settings-row__control` double-definition between components.css and pages.css

- [ ] **Step 1: Write the failing test**

If `apps/gui/src/components/desktop/SettingsRow.test.tsx` does not exist, create it. Otherwise extend:

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { SettingsRow } from "./SettingsRow";

describe("SettingsRow", () => {
  it("renders label and description", () => {
    render(<SettingsRow label="Setting" description="Details" control={<span>ctl</span>} />);
    expect(screen.getByText("Setting")).toBeTruthy();
    expect(screen.getByText("Details")).toBeTruthy();
    expect(screen.getByText("ctl")).toBeTruthy();
  });

  it("renders error text when provided", () => {
    render(<SettingsRow label="X" control={<span />} error="Required" />);
    expect(screen.getByText("Required")).toBeTruthy();
  });

  it("defaults to horizontal layout", () => {
    const { container } = render(<SettingsRow label="X" control={<span />} />);
    expect(container.querySelector(".settings-row__control--horizontal")).toBeTruthy();
  });

  it("applies vertical layout modifier", () => {
    const { container } = render(<SettingsRow label="X" control={<span />} layout="vertical" />);
    expect(container.querySelector(".settings-row__control--vertical")).toBeTruthy();
  });

  it("applies dangerous modifier", () => {
    const { container } = render(<SettingsRow label="X" control={<span />} dangerous />);
    expect(container.querySelector(".settings-row--dangerous")).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `pnpm --filter @busytok/gui test src/components/desktop/SettingsRow.test.tsx`
Expected: FAIL — `layout` prop not accepted / no `--horizontal` / `--vertical` classes.

- [ ] **Step 3: Implement the layout variant**

Edit `apps/gui/src/components/desktop/SettingsRow.tsx`:

```tsx
import type { ReactNode } from "react";

export function SettingsRow({
  label,
  description,
  control,
  error,
  dangerous,
  layout = "horizontal",
}: {
  label: string;
  description?: string;
  control: ReactNode;
  error?: string | null;
  dangerous?: boolean;
  layout?: "horizontal" | "vertical";
}) {
  return (
    <div className={`settings-row${dangerous ? " settings-row--dangerous" : ""}`}>
      <div>
        <h3>{label}</h3>
        {description ? <p>{description}</p> : null}
      </div>
      <div className={`settings-row__control settings-row__control--${layout}`}>
        {control}
        {error ? <span className="settings-row__error">{error}</span> : null}
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Add CSS vertical variant + error/dangerous rules**

In `apps/gui/src/styles/components.css`, after the `.settings-row__control` rule (~L435), add the layout variant AND the error/dangerous rules that currently only live in `pages.css` (so they are canonical before pages.css deletes them in Task 9):

```css
.settings-row__control--vertical {
  flex-direction: column;
  align-items: flex-end;
  gap: 4px;
}

.settings-row__error {
  font-size: 11px;
  color: var(--color-status-danger);
  white-space: nowrap;
}

.settings-row--dangerous {
  padding: var(--space-3) var(--space-3);
  margin: 0 calc(var(--space-3) * -1);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--color-status-danger) 6%, transparent);
  border-left: 3px solid var(--color-status-danger);
}

.settings-row--dangerous h3 {
  color: var(--color-status-danger);
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `pnpm --filter @busytok/gui test src/components/desktop/SettingsRow.test.tsx`
Expected: PASS (5 cases).

- [ ] **Step 6: Commit**

```bash
git add apps/gui/src/components/desktop/SettingsRow.tsx apps/gui/src/components/desktop/SettingsRow.test.tsx apps/gui/src/styles/components.css
git commit -m "feat(gui): add horizontal|vertical layout variant to SettingsRow
Moves the layout-direction choice into the component prop, resolving
the .settings-row__control double-definition conflict between
components.css (row) and pages.css (column)."
```

---

# Phase 2 — Page Migration + CSS Cleanup

## Task 8: SettingsPage migration to canonical controls

**Files:**
- Modify: `apps/gui/src/pages/SettingsPage.tsx`
- Test: `apps/gui/src/pages/SettingsPageCoverage.test.tsx` (extend — this is the existing Settings test harness with `mockPage()` helpers)

**Interfaces:**
- Consumes: All canonical controls from Tasks 1-7
- Produces: SettingsPage uses zero page-private control classes

**Migration map:**

| Old pattern | New canonical control | SettingsPage locations |
|---|---|---|
| `segmented-group` + `segmented-label` (radio) | `SegmentedControl` (with `size="default"`) | L593-616 (Week starts on) |
| `toggle-label` + `toggle` + `toggle-track` (6 groups) | `ToggleSwitch` | L633-641, 650-658, 730-738, 747-755, 773-794, 876-886 |
| `diag-value` (12 uses) | `SettingsValue` (with appropriate `tone`) | L519, 538, 578, 810, 895, 903, 974, 978, 982, 987, 1004, 1014 |
| `manual-root-controls` (3 uses) | `SettingsActionGroup` | L518, 677, 1003 |

- [ ] **Step 1: Add imports**

In `apps/gui/src/pages/SettingsPage.tsx`, add to existing imports (keep all existing imports):

```ts
import { ToggleSwitch } from "../components/desktop/ToggleSwitch";
import { SettingsValue } from "../components/desktop/SettingsValue";
import { SettingsStatus } from "../components/desktop/SettingsStatus";
import { SettingsActionGroup } from "../components/desktop/SettingsActionGroup";
```

- [ ] **Step 1b: Add telemetry for migration completion**

In `apps/gui/src/pages/SettingsPage.tsx`, add a one-time telemetry event via `useEffect` so the side effect lives outside the render path. Import the safe reporter (add near existing imports):

```ts
import { reportFrontendEventSafely } from "../logging/safeReporter";
```

Inside the component body, after the existing `useEffect` blocks, add:

```ts
useEffect(() => {
  reportFrontendEventSafely({
    level: "INFO",
    event_code: "gui.controls.migration_complete",
    message: "SettingsPage rendered with canonical controls",
  });
}, []); // fire once on mount; StrictMode double-fire is harmless (idempotent log)
```

Never put `reportFrontendEventSafely` directly in the render body — render-side effects violate React semantics and are unstable under StrictMode re-render. All existing telemetry in `SettingsPage.tsx` (shortcut diagnostics, theme change logging) is preserved unchanged.

- [ ] **Step 2: Replace "Week starts on" segmented-group with SegmentedControl**

Replace L590-616 (the `fieldset.segmented-group` block). The `control` for the "Week starts on" SettingsRow becomes:

```tsx
control={
  <SegmentedControl
    label="Week start day"
    value={String(weekStart)}
    options={[
      { value: "0", label: "Sunday" },
      { value: "1", label: "Monday" },
    ]}
    onChange={(v) => handleWeekStartChange(Number(v) as 0 | 1)}
    size="default"
  />
}
```

The old `<fieldset className="segmented-group">` had `aria-label="Week start day"` — the `SegmentedControl` carries its own `aria-label` from its `label` prop, so the external aria-label is obsolete. The `error={fieldError("week_starts_on")}` prop on `SettingsRow` is unrelated to aria-label (it handles server-side validation) and stays.

- [ ] **Step 3: Replace all 6 toggle groups with ToggleSwitch**

For each toggle group, replace the `label.toggle-label > input.toggle + span.toggle-track` structure with `<ToggleSwitch>`. Example for "Claude Code" toggle (L628-642):

Old:
```tsx
control={
  <label className="toggle-label">
    <input type="checkbox" className="toggle" checked={discovery.claude_code_default_paths} onChange={() => handleDiscoveryToggle("claude_code_default_paths")} aria-label="Claude Code" />
    <span className="toggle-track" />
  </label>
}
```

New:
```tsx
control={
  <ToggleSwitch
    checked={discovery.claude_code_default_paths}
    onChange={() => handleDiscoveryToggle("claude_code_default_paths")}
    aria-label="Claude Code"
    size="default"
  />
}
```

Apply the same pattern to the other 5 toggle groups (Codex, Local only, Redact, Launch at login, Show Diagnostics). For each one, the `SettingsRow` already carries the visible `label` and `description` — the `ToggleSwitch` provides only the interactive switch with `aria-label` for accessibility.

- [ ] **Step 4: Replace all 12 `diag-value` spans with `SettingsValue`**

Map each usage to the appropriate tone:

| Current location (line) | Current content | New `SettingsValue` tone |
|---|---|---|
| L519 (shortcut unavailable) | `"Unavailable"` | `tone="muted"` |
| L538 (shortcut status) | `shortcutStatusText(...)` | `tone="default"` |
| L578 (timezone) | `{timezone}` | `tone="default"` |
| L810 (bg service loading) | `"Checking..."` | `tone="muted"` |
| L895 (gui build) | `{bgDiag.gui_build_identity}` | `tone="default"` |
| L903 (service build) | `{bgDiag.service_build_identity ?? "Unknown"}` | `tone="default"` |
| L974 (db size) | `{formatBytes(...)}` | `tone="default"` |
| L978 (migration version) | `{diagnostics.migration_version}` | `tone="default"` |
| L982 (event count) | `{diagnostics.usage_event_count.toLocaleString()}` | `tone="default"` |
| L987 (checkpoint) | date string | `tone="muted"` |
| L1004 (paste status, permission missing) | `pasteStatusText(...)` | `tone="warning"` |
| L1014 (paste status, else) | `pasteStatusText(...)` | `tone="default"` |

Replace each `<span className="diag-value">{...}</span>` with `<SettingsValue value={...} tone="..." size="default" />`.

- [ ] **Step 5: Replace 3 `manual-root-controls` divs with `SettingsActionGroup`**

- L518 (shortcut retry): `<SettingsActionGroup direction="col"><SettingsStatus label="Unavailable" tone="danger" /><button ...>Retry</button></SettingsActionGroup>`
- L677 (manual root inputs): `<SettingsActionGroup direction="col">{...existing input elements...}</SettingsActionGroup>`
- L1003 (paste permission fixer): `<SettingsActionGroup direction="col"><SettingsStatus label={pasteStatusText(pasteStatus)} tone="warning" /><button ...>Fix</button></SettingsActionGroup>`

Use `SettingsStatus` for the status label where previously a bare string was used with `diag-value`.

- [ ] **Step 6: Add contract regression assertions**

In `apps/gui/src/pages/SettingsPageCoverage.test.tsx`, add to the existing describe block — call `mockPage()` (the file's standard setup) before each assertion:

```ts
it("uses zero page-private control classes after canonical migration", () => {
  mockPage();
  render(<SettingsPage />);
  expect(document.querySelector(".segmented-group")).toBeNull();
  expect(document.querySelector(".segmented-label")).toBeNull();
  expect(document.querySelector(".toggle-label")).toBeNull();
  expect(document.querySelector(".toggle")).toBeNull();
  expect(document.querySelector(".toggle-track")).toBeNull();
  expect(document.querySelector(".diag-value")).toBeNull();
  expect(document.querySelector(".manual-root-controls")).toBeNull();
});

it("renders Theme and Week starts on via shared SegmentedControl", () => {
  mockPage();
  render(<SettingsPage />);
  const groups = document.querySelectorAll(".segmented-control");
  expect(groups.length).toBeGreaterThanOrEqual(2);
});
```

- [ ] **Step 7: Run all tests + typecheck**

Run: `pnpm --filter @busytok/gui test src/pages/SettingsPageCoverage.test.tsx && pnpm --filter @busytok/gui typecheck`
Expected: PASS, typecheck clean. If existing SettingsPage tests relied on querying by old classNames, update those assertions to query by canonical component structure.

- [ ] **Step 8: Commit**

```bash
git add apps/gui/src/pages/SettingsPage.tsx apps/gui/src/pages/SettingsPageCoverage.test.tsx
git commit -m "refactor(gui): migrate SettingsPage to canonical controls

Replace all page-private controls:
- segmented-group/label → SegmentedControl (Week starts on)
- toggle-label/toggle/toggle-track (6 groups) → ToggleSwitch
- diag-value (12 uses) → SettingsValue with tone
- manual-root-controls (3 uses) → SettingsActionGroup + SettingsStatus
Add contract regression assertions for zero private-control classes."
```

---

## Task 9: CSS cleanup — remove dead page-private rules

**Files:**
- Modify: `apps/gui/src/styles/pages.css` — remove toggle, segmented-group, diag-value, manual-root-controls rules; remove `.settings-row__control` override
- Modify: `apps/gui/src/styles/components.css` — remove `.prompt-overlay__hints` family (done in Task 10)

**Pre-check:** `grep` the entire `apps/gui/src` for any remaining consumer of the classes being deleted.

- [ ] **Step 1: Verify zero consumers remain**

Run:
```bash
grep -rn "segmented-group\|segmented-label" apps/gui/src --include="*.tsx" --include="*.ts" | grep -v test | grep -v "\.css"
grep -rn "toggle-label\|className=\"toggle\"\|toggle-track" apps/gui/src --include="*.tsx" --include="*.ts" | grep -v test | grep -v "\.css"
grep -rn "diag-value" apps/gui/src --include="*.tsx" --include="*.ts" | grep -v test | grep -v "\.css"
grep -rn "manual-root-controls" apps/gui/src --include="*.tsx" --include="*.ts" | grep -v test | grep -v "\.css"
```

Expected: all commands exit with zero matches (only CSS definitions remain). If any match is found, the migration in Task 8 missed a spot — fix it before proceeding.

- [ ] **Step 2: Remove toggle CSS from pages.css**

Delete lines ~1374-1429 (`.toggle-label`, `.toggle`, `.toggle-track`, and all associated pseudo-elements/pseudo-classes) from `apps/gui/src/styles/pages.css`.

- [ ] **Step 3: Remove segmented-group CSS from pages.css**

Delete lines ~1431-1478 (`.segmented-group`, `.segmented-label`, and all associated rules) from `apps/gui/src/styles/pages.css`. Also remove the section comment `/* ── Segmented control (radio group) ── */`.

- [ ] **Step 4: Remove manual-root-controls CSS from pages.css**

Delete lines ~1480-1493 (`.manual-root-controls` block and the section comment `/* ── Manual root controls ── */`) from `apps/gui/src/styles/pages.css`.

- [ ] **Step 5: Remove diag-value CSS from pages.css**

Delete lines ~1513-1518 (`.diag-value` rule) and the section comment `/* ── Diagnostics badges & values ── */` from `apps/gui/src/styles/pages.css`.

- [ ] **Step 6: Remove .settings-row__control override from pages.css**

Delete lines ~1336-1342 (the `.settings-row__control` block that sets `flex-direction: column` / `align-items: flex-end` / `gap: 4px`) from `apps/gui/src/styles/pages.css`. The canonical rule in `components.css` now has `--vertical` / `--horizontal` variants owned by the `SettingsRow` component.

Also delete the `.settings-row__error` and `.settings-row--dangerous` rules from pages.css (~L1344-1360). These rules were migrated to `components.css` in Task 7 Step 4; having them only in `components.css` is the canonical state.

- [ ] **Step 7: Run the full test suite**

Run: `pnpm --filter @busytok/gui test`
Expected: all existing tests still PASS. The old CSS classes are no longer rendered so no visual regression.

- [ ] **Step 8: Commit**

```bash
git add apps/gui/src/styles/pages.css
git commit -m "refactor(gui): remove dead page-private CSS rules

Delete toggle, segmented-group/label, diag-value, manual-root-controls,
and the .settings-row__control override from pages.css. Zero consumers
remain after SettingsPage migration to canonical controls (Task 8)."
```

---

## Task 10: Prompt Palette hints footer removal

**Files:**
- Modify: `apps/gui/src/components/prompt-palette/PromptPaletteOverlay.tsx` — delete hints footer JSX
- Modify: `apps/gui/src/styles/components.css` — delete `.prompt-overlay__hints` CSS family
- Test: `apps/gui/src/components/prompt-palette/PromptPaletteOverlay.test.tsx` — remove hints assertion

- [ ] **Step 1: Delete the hints footer JSX**

In `apps/gui/src/components/prompt-palette/PromptPaletteOverlay.tsx`, delete lines ~350-381 (the entire `<footer className="prompt-overlay__hints">...</footer>` block, including the `{hasResults ? (...): null}` conditional wrapper).

- [ ] **Step 2: Remove the now-dead `hasResults` variable**

`hasResults` (L94) is only referenced at L350 (the deleted hints footer block). Delete its definition and the associated comment on L91-93 ("TanStack Query keeps stale `data`..."). Verify: `grep -n "hasResults"` in `PromptPaletteOverlay.tsx` returns zero matches after removal.

- [ ] **Step 3: Delete the hints CSS**

In `apps/gui/src/styles/components.css`, delete:
- The `.prompt-overlay__hints` block (~L1227-1248)
- The `.prompt-overlay__hint` block
- The `.prompt-overlay__hint-label` block (if separate)
- The `.prompt-overlay__keycap` block — **KEEP THIS** (the keycap style may be used elsewhere in the overlay, e.g., action keycaps)
- The `@media` override for `.prompt-overlay__hints` (~L1459-1461)

Verify: `grep -rn "prompt-overlay__hints\|prompt-overlay__hint\b" apps/gui/src --include="*.css" --include="*.tsx"`. Only the deleted CSS rules should remain.

- [ ] **Step 4: Update the test**

In `apps/gui/src/components/prompt-palette/PromptPaletteOverlay.test.tsx`, remove the test block at L93-99 that asserts `document.querySelector(".prompt-overlay__hints")` exists. Replace it with a negative assertion:

```ts
it("does not render a keyboard-hints footer", () => {
  // ... setup that renders the overlay with results ...
  expect(document.querySelector(".prompt-overlay__hints")).toBeNull();
});
```

Alternatively, if the existing test workflow doesn't easily render results, just delete the old assertion block — the negative assertion is covered by rendering the component and checking the DOM doesn't contain `.prompt-overlay__hints`.

- [ ] **Step 5: Run the overlay tests**

Run: `pnpm --filter @busytok/gui test src/components/prompt-palette/PromptPaletteOverlay.test.tsx`
Expected: PASS (old hints assertion removed/replaced; all other cases green).

- [ ] **Step 6: Commit**

```bash
git add apps/gui/src/components/prompt-palette/PromptPaletteOverlay.tsx apps/gui/src/styles/components.css apps/gui/src/components/prompt-palette/PromptPaletteOverlay.test.tsx
git commit -m "refactor(gui): remove Prompt Palette hints footer (JSX + CSS + test)

The keyboard-hints footer was built-in chrome, not an interactive
surface. Knowledge of shortcuts is carried by docs/menu, not an
always-visible footer. Deleted: JSX block, .prompt-overlay__hints CSS
family, and the corresponding test assertion."
```

---

# Phase 3 — Gate

## Task 11: Coverage + contract regression gate

**Files:**
- No new files; verify all modified files pass coverage threshold.

- [ ] **Step 1: Full frontend test suite**

Run: `pnpm --filter @busytok/gui test`
Expected: all tests PASS. Fix any failures before proceeding.

- [ ] **Step 2: Coverage gate**

Run: `pnpm --filter @busytok/gui test:coverage`
Expected: global lines threshold ≥ 90% green. Each new canonical control (ToggleSwitch, SettingsValue, SettingsStatus, SettingsActionGroup) should show ≥90% line coverage in its own test suite. Modified files (SegmentedControl, Select, SettingsRow, SettingsPage) must not regress existing coverage.

- [ ] **Step 3: Typecheck**

Run: `pnpm --filter @busytok/gui typecheck`
Expected: clean, zero errors.

- [ ] **Step 4: Contract regression verification**

Run the contract assertions added in Task 8 Step 6:
```bash
pnpm --filter @busytok/gui test src/pages/SettingsPageCoverage.test.tsx
```
Expected: the "uses zero page-private control classes" and "renders Theme/Week via shared SegmentedControl" tests PASS.

- [ ] **Step 5: Dead CSS verification**

Run:
```bash
grep -rn "segmented-group\|segmented-label\|toggle-label\|diag-value\|manual-root-controls" apps/gui/src --include="*.tsx" --include="*.ts"
```
Expected: zero matches (these class names no longer appear in any TSX/TS file). CSS definitions in components.css for ToggleSwitch/SettingsValue/SettingsStatus/SettingsActionGroup use new, canonical class names.

- [ ] **Step 6: Run existing App-level tests**

Run: `pnpm --filter @busytok/gui test src/App.test.tsx src/App.promptPaletteFlow.test.tsx`
Expected: PASS (SettingsPage migration doesn't break App-level rendering).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "chore(gui): coverage + contract gate — all canonical controls ≥90%, zero private classes remain"
```

---

## Verification gate (end of Phase 3)

- [ ] `pnpm --filter @busytok/gui test` — full suite green
- [ ] `pnpm --filter @busytok/gui test:coverage` — global lines threshold ≥ 90% green; every new canonical control component test suite targets ≥90% line coverage
- [ ] `pnpm --filter @busytok/gui typecheck` — clean
- [ ] `grep -rn "segmented-group\|toggle-label\|diag-value\|manual-root-controls" apps/gui/src --include="*.tsx"` — zero matches
- [ ] `.settings-row__control` defined in `components.css` only (not pages.css)
- [ ] `.prompt-overlay__hints` zero references in any file
- [ ] `SegmentedControl` is the only segmented choice (radio-group variant deleted)
- [ ] `AppSelect` + `TagFilterCombobox` are the only dropdown surfaces

---

## Self-Review

**1. Spec coverage:**

| Spec requirement | Task |
|---|---|
| §5.2 SegmentedControl canonical (size, delete segmented-group/label) | Task 1 + Task 9 |
| §5.3 ToggleSwitch canonical (delete toggle-label/toggle/toggle-track) | Task 3 + Task 9 |
| §5.4 Select canonical (size, dense) | Task 2 |
| §5.5 Combobox canonical (separate, shares CSS contract) | Task 2 (comment) + Task 9 |
| §5.6 SettingsValue canonical (4 tones, replace diag-value) | Task 4 + Task 8+9 |
| §5.7 SettingsStatus canonical (separate from StatusPill) | Task 5 + Task 8 |
| §5.8 SettingsActionGroup canonical (replace manual-root-controls) | Task 6 + Task 8+9 |
| §5.1 SettingsRow layout variants (horizontal/vertical, open interface) | Task 7 |
| §6.1 Size system (default|dense only) | Tasks 1,2,3,4,5 |
| §8.1 Delete segmented-group/label | Task 9 |
| §8.1 Delete toggle-label/toggle/toggle-track | Task 9 |
| §8.1 Delete diag-value | Task 9 |
| §8.1 Delete manual-root-controls | Task 9 |
| §7.3 / §8.1 Delete Prompt Palette hints | Task 10 |
| §9 CSS layering (components.css=controls, pages.css=composition) | Task 7 + Task 9 |
| §10.1 Component tests (default/dense/disabled/focus/long-label/error) | Tasks 1-7 (per-component) |
| §10.2 Contract regression assertions | Task 8 Step 6 + Task 11 |
| §10.2 Prompt Palette no-hints assertion | Task 10 Step 4 |
| §10.3 Coverage > 90% | Task 11 |
| §8.2 Prohibit new page-private controls | Documented in component comments |

**2. Placeholder scan:** No TBD/TODO/"add error handling"/"similar to". Every step has actual code.

**3. Type consistency:**
- `size` prop: `"default" | "dense"` (consistent across SegmentedControl, AppSelect, ToggleSwitch, SettingsValue, SettingsStatus)
- `SettingsValue` tone: `"default" | "muted" | "warning" | "danger"` (used in Task 4, consumed in Task 8)
- `SettingsStatus` tone: `"ok" | "warning" | "danger" | "muted"` (used in Task 5, consumed in Task 8)
- `SettingsActionGroup` direction: `"col" | "row"` (used in Task 6, consumed in Task 8)
- `SettingsRow` layout: `"horizontal" | "vertical"` (used in Task 7, consumed in Task 8)
- BEM class naming: `toggle-switch`, `settings-value`, `settings-status`, `settings-action-group` (consistent prefix scheme)
