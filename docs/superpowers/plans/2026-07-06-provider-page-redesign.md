# Provider Page Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign the Provider page UI for simplicity, delete Profiles UI, fold Models into always-expanded Provider cards with inline create/edit, and add `busytok provider` CLI parity for all provider+model CRUD operations.

**Architecture:** React 19 + Tauri 2 + TanStack Query v5 frontend with a custom CSS design system. Rust CLI using `clap` derive + `ControlClient` RPC. Reuses all existing `provider.*` / `model.*` RPCs — no backend changes. The frontend reuses existing `useProviders` / `useProviderMutations` / `useModels` / `useModelMutations` hooks from [useBusytokData.ts](file:///Users/wsd/Data/Busytok/busytok/apps/gui/src/api/useBusytokData.ts); only `useProfileMutations` and the `profile*` client methods are deleted. The CLI mirrors the established [commands/models.rs](file:///Users/wsd/Data/Busytok/busytok/apps/cli/src/commands/models.rs) pattern (in-process `ControlServer` + `RuntimeControl` test double).

**Tech Stack:** React 19, TanStack Query v5, TypeScript 5, Vitest + @testing-library/react, Rust 1.88, clap 4, tokio, serde_json.

## Global Constraints

- Reuse all existing RPCs (`provider.list/create/update/delete/test_connection`, `model.list/create/update/delete/tags.update`). No backend changes.
- No new npm dependencies. No new Rust crates — `url` is NOT a workspace dep; URL host extraction uses manual string parsing.
- CSS must use existing `--color-*` tokens from [tokens.css](file:///Users/wsd/Data/Busytok/busytok/apps/gui/src/styles/tokens.css). Do NOT introduce `--border-color`, `--surface-bg`, `--text-muted`, `--hover-bg`, `--chip-bg`, or `--chip-text` — they do not exist.
- Observability: every provider/model CRUD success AND failure path must emit `reportFrontendEventSafely` events per spec §8. Tests must assert event codes via `expect.objectContaining({ event_code: ... })`.
- Test coverage ≥ 90% for new and modified code.
- Code quality: abstract shared utilities (`parseTags`, `deriveProviderName`, `validateBaseUrl`) into a single module; delete dead code completely (`ProfilesSection`, `ModelsSection`, `useProfileMutations`, `profile*` client methods).
- Rust: `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test --workspace` must pass.
- Frontend: `pnpm --filter @busytok/gui test`, `pnpm --filter @busytok/gui typecheck`, `pnpm --filter @busytok/gui build` must pass. Coverage gate: `pnpm coverage:gui` (enforces ≥90% line coverage via `--coverage.thresholds.lines 90`).
- API Key is required for provider creation (both UI and CLI).
- Provider `name` is auto-derived from Base URL + Kind; not a form field.
- Three-state `api_key` contract on `ProviderUpdateRequestDto`: `undefined` = no change, `null` = clear, `string` = update. Preserve this in the edit form.
- Model mutations target `model_db_id` (SQL PK), not `model_id` (human-readable string).
- `ModelUpdateRequestDto` fields are single-state `Option<T>` — no clear-to-null semantics. Empty `display_name` input = "leave unchanged".
- CLI destructive commands (`provider delete`, `provider model delete`) require `--yes` flag in non-interactive mode; prompt only on TTY.
- The existing `busytok models` command stays unchanged (backward compat).

---

## File Structure

### Deleted
- `apps/gui/src/components/ProfilesSection.tsx`
- `apps/gui/src/components/ProfilesSection.test.tsx`
- `apps/gui/src/components/ModelsSection.tsx`
- `apps/gui/src/components/ModelsSection.test.tsx`

### Created (GUI)
- `apps/gui/src/pages/providerFormUtils.ts` — pure helpers: `parseTags`, `deriveProviderName`, `deriveUniqueProviderName`, `validateBaseUrl`
- `apps/gui/src/pages/providerFormUtils.test.ts` — unit tests for the above
- `apps/gui/src/components/ProviderCard.tsx` — always-expanded card with inline model list + edit
- `apps/gui/src/components/ProviderCard.test.tsx`

### Created (CLI)
- `apps/cli/src/commands/provider.rs` — all provider+model CLI handlers (with in-source tests)

### Modified (GUI)
- `apps/gui/src/pages/ProvidersPage.tsx` — full rewrite: new creation form + ProviderCard list
- `apps/gui/src/pages/ProvidersPage.test.tsx` — update tests for new structure + event assertions
- `apps/gui/src/api/useBusytokData.ts` — delete `useProfileMutations` + Profile DTO imports
- `apps/gui/src/api/busytokClient.ts` — delete `profileCreate/Update/Delete` + Profile DTO imports
- `apps/gui/src/styles/pages.css` — add `.provider-card`, `.model-row`, `.chip` classes

### Modified (CLI)
- `apps/cli/src/main.rs` — add `ProviderCommand` + `ProviderModelCommand` enums, `Command::Provider` variant, dispatch arm, `command_name` arm
- `apps/cli/src/commands/mod.rs` — add `pub mod provider;`

---

## Task 1: Delete Profiles UI (Frontend Cleanup)

**Files:**
- Delete: `apps/gui/src/components/ProfilesSection.tsx`
- Delete: `apps/gui/src/components/ProfilesSection.test.tsx`
- Modify: `apps/gui/src/pages/ProvidersPage.tsx:12` (remove import), `:703` (remove render)
- Modify: `apps/gui/src/api/useBusytokData.ts:45-46` (remove Profile DTO imports), `:497-517` (delete `useProfileMutations`)
- Modify: `apps/gui/src/api/busytokClient.ts:44-46` (remove Profile DTO imports), `:210-215` (delete profile methods)
- Modify: `apps/gui/src/pages/ProvidersPage.test.tsx` (remove `useProfileMutations` + `useSettingsSnapshot` mocks if no longer needed)

**Interfaces:**
- Consumes: existing `useProviders`, `useProviderMutations` (unchanged)
- Produces: cleaned-up `ProvidersPage` that no longer references `ProfilesSection`

- [ ] **Step 1: Delete ProfilesSection files**

```bash
rm apps/gui/src/components/ProfilesSection.tsx apps/gui/src/components/ProfilesSection.test.tsx
```

- [ ] **Step 2: Remove ProfilesSection imports and render from ProvidersPage**

In `apps/gui/src/pages/ProvidersPage.tsx`:
- Delete line 12: `import { ProfilesSection } from "../components/ProfilesSection";`
- Delete line 703: `        <ProfilesSection />`

- [ ] **Step 3: Delete `useProfileMutations` from useBusytokData.ts**

In `apps/gui/src/api/useBusytokData.ts`:
- Delete the import lines for `ProfileCreateRequestDto` and `ProfileUpdateRequestDto` (currently at lines 45-46 inside the `import type { ... } from "@busytok/protocol-types";` block).
- Delete the entire `useProfileMutations` function (lines 497-517, including the doc comment above it if any).

- [ ] **Step 4: Delete profile client methods from busytokClient.ts**

In `apps/gui/src/api/busytokClient.ts`:
- Delete the import lines for `ProfileCreateRequestDto`, `ProfileUpdateRequestDto`, and `ProfileDto` (lines 44-46). Keep `ModelCatalogEntryDto` (line 47) and everything below.
- Delete the three profile methods (lines 210-215): `profileCreate`, `profileUpdate`, `profileDelete`. Also delete the comment block at lines 207-209 that introduces the profile section.

- [ ] **Step 5: Remove `useProfileMutations` mock from ProvidersPage.test.tsx**

In `apps/gui/src/pages/ProvidersPage.test.tsx`:
- Remove `useProfileMutations` from the `vi.mock("../api/useBusytokData", ...)` factory (line 23 area).
- Remove `useProfileMutations` from the imports (line 49 area).
- Remove `const mockUseProfileMutations = vi.mocked(useProfileMutations);` (around line 56).
- Remove `mockUseProfileMutations.mockReturnValue(...)` from `beforeEach` (lines 170-174).
- If `useSettingsSnapshot` was only mocked for `ProfilesSection`, remove it too. (Check whether `ProvidersPage` itself or any remaining child uses `useSettingsSnapshot` — if not, remove its mock and import as well.)

- [ ] **Step 6: Run tests to verify nothing breaks**

Run: `pnpm --filter @busytok/gui test -- --run ProvidersPage`
Expected: PASS. If failures reference `useProfileMutations` or `ProfilesSection`, search for stragglers.

- [ ] **Step 7: Run typecheck and build**

Run: `pnpm --filter @busytok/gui typecheck && pnpm --filter @busytok/gui build`
Expected: PASS with no errors.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(gui): delete Profiles UI and useProfileMutations

Per provider page redesign spec §2. Profiles backend RPCs are out of
scope and remain. Removes ProfilesSection.tsx/.test.tsx, the
useProfileMutations hook, and profile* client methods."
```

---

## Task 2: Add CSS Classes for Card UI

**Files:**
- Modify: `apps/gui/src/styles/pages.css` (append new classes at the end of the file)

**Interfaces:**
- Produces: `.provider-card`, `.provider-card__*`, `.model-row`, `.chip` classes for Tasks 4-8

Note: CSS is visual and not unit-testable per spec §10. This task has no TDD cycle.

- [ ] **Step 1: Append the new CSS classes to pages.css**

Append to the end of `apps/gui/src/styles/pages.css`:

```css
/* ─── Provider page redesign (always-expanded cards) ─────────────────────── */

.provider-card {
  border: 1px solid var(--color-border-subtle);
  border-radius: 8px;
  background: var(--color-surface);
  margin-bottom: 12px;
  transition: box-shadow 0.15s ease;
}
.provider-card:hover {
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.06);
}
.provider-card__header {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 14px 16px;
  border-bottom: 1px solid var(--color-border-subtle);
}
.provider-card__name {
  font-weight: 600;
  font-size: 0.95rem;
  color: var(--color-text);
}
.provider-card__info {
  padding: 8px 16px;
  font-size: 0.85rem;
  color: var(--color-text-muted);
}
.provider-card__models {
  padding: 12px 16px;
}
.provider-card__models--disabled {
  opacity: 0.5;
  pointer-events: none;
}
.provider-card__notice {
  padding: 6px 16px;
  font-size: 0.8rem;
  color: var(--color-status-warning);
  background: var(--color-status-warning-soft);
}
.provider-card__error-banner {
  padding: 8px 16px;
  font-size: 0.85rem;
  color: var(--color-status-danger);
  background: var(--color-status-danger-soft);
  border-radius: 4px;
  margin: 8px 16px;
}
.model-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  border-radius: 6px;
  transition: background 0.1s ease;
}
.model-row:hover {
  background: var(--color-hover);
}
.model-row__name {
  font-weight: 500;
  color: var(--color-text);
}
.model-row__tags {
  display: inline-flex;
  gap: 4px;
  flex-wrap: wrap;
}
.chip {
  display: inline-flex;
  align-items: center;
  padding: 2px 8px;
  border-radius: 4px;
  font-size: 0.75rem;
  background: var(--color-surface-subtle);
  color: var(--color-text-muted);
}
.chip--kind {
  background: var(--color-status-info-soft);
  color: var(--color-text);
}
```

- [ ] **Step 2: Verify build still passes**

Run: `pnpm --filter @busytok/gui build`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add apps/gui/src/styles/pages.css
git commit -m "feat(gui): add provider-card / model-row / chip CSS classes

Uses existing --color-* tokens from tokens.css. Per spec §6."
```

---

## Task 3: Extract Shared Provider Form Utilities (TDD)

**Files:**
- Create: `apps/gui/src/pages/providerFormUtils.ts`
- Test: `apps/gui/src/pages/providerFormUtils.test.ts`

**Interfaces:**
- Produces (consumed by Tasks 4, 6, 7 and CLI equivalents are separate):
  - `parseTags(input: string): string[]` — split on comma, trim, drop empties
  - `deriveProviderName(url: string, kind: string): string` — `{domain}_{kindShort}` from URL hostname
  - `deriveUniqueProviderName(url: string, kind: string, existingNames: Set<string>): string` — append `_2`, `_3`, ... on collision
  - `validateBaseUrl(input: string): string | null` — returns error message or null if valid

- [ ] **Step 1: Write the failing tests**

Create `apps/gui/src/pages/providerFormUtils.test.ts`:

```typescript
import { describe, expect, it } from "vitest";
import {
  deriveProviderName,
  deriveUniqueProviderName,
  parseTags,
  validateBaseUrl,
} from "./providerFormUtils";

describe("parseTags", () => {
  it("returns empty array for empty string", () => {
    expect(parseTags("")).toEqual([]);
  });
  it("trims and splits on comma", () => {
    expect(parseTags("cheap, fast , reasoning")).toEqual(["cheap", "fast", "reasoning"]);
  });
  it("drops empty entries", () => {
    expect(parseTags("cheap,,fast,")).toEqual(["cheap", "fast"]);
  });
  it("handles whitespace-only entries", () => {
    expect(parseTags("  ,  ")).toEqual([]);
  });
});

describe("deriveProviderName", () => {
  it("derives domain_kind from a typical URL", () => {
    expect(deriveProviderName("https://api.deepseek.com/v1", "openai_compatible"))
      .toBe("deepseek_openai");
  });
  it("strips _compatible suffix from kind", () => {
    expect(deriveProviderName("https://api.anthropic.com", "anthropic_compatible"))
      .toBe("anthropic_anthropic");
  });
  it("falls back to full host for single-part hostnames", () => {
    expect(deriveProviderName("https://localhost:8080/v1", "openai_compatible"))
      .toBe("localhost_openai");
  });
  it("handles URL with port", () => {
    expect(deriveProviderName("http://my.api.host:3000", "openai_compatible"))
      .toBe("host_openai");
  });
});

describe("deriveUniqueProviderName", () => {
  it("returns base name when no collision", () => {
    const existing = new Set<string>(["other_openai"]);
    expect(deriveUniqueProviderName("https://api.deepseek.com", "openai_compatible", existing))
      .toBe("deepseek_openai");
  });
  it("appends _2 on first collision", () => {
    const existing = new Set<string>(["deepseek_openai"]);
    expect(deriveUniqueProviderName("https://api.deepseek.com", "openai_compatible", existing))
      .toBe("deepseek_openai_2");
  });
  it("increments suffix until unique", () => {
    const existing = new Set<string>(["deepseek_openai", "deepseek_openai_2", "deepseek_openai_3"]);
    expect(deriveUniqueProviderName("https://api.deepseek.com", "openai_compatible", existing))
      .toBe("deepseek_openai_4");
  });
});

describe("validateBaseUrl", () => {
  it("returns null for valid https URL", () => {
    expect(validateBaseUrl("https://api.deepseek.com/v1")).toBeNull();
  });
  it("returns null for valid http URL", () => {
    expect(validateBaseUrl("http://localhost:8080")).toBeNull();
  });
  it("returns error for empty input", () => {
    expect(validateBaseUrl("")).toBe("Base URL 不能为空");
  });
  it("returns error for whitespace-only input", () => {
    expect(validateBaseUrl("   ")).toBe("Base URL 不能为空");
  });
  it("returns error for missing protocol", () => {
    expect(validateBaseUrl("api.deepseek.com")).toBe("请输入完整的 URL（以 http:// 或 https:// 开头）");
  });
  it("returns error for ftp protocol", () => {
    expect(validateBaseUrl("ftp://api.deepseek.com")).toBe("请输入完整的 URL（以 http:// 或 https:// 开头）");
  });
  it("returns error for malformed URL", () => {
    expect(validateBaseUrl("https://")).toBe("URL 格式不正确");
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --filter @busytok/gui test -- --run providerFormUtils`
Expected: FAIL with "Cannot find module './providerFormUtils'".

- [ ] **Step 3: Write the implementation**

Create `apps/gui/src/pages/providerFormUtils.ts`:

```typescript
/**
 * Shared helpers for provider creation/edit forms.
 * Used by ProvidersPage (GUI) — the CLI has a Rust mirror in
 * apps/cli/src/commands/provider.rs.
 */

/** Split a comma-separated tag string into a clean string[]. */
export function parseTags(input: string): string[] {
  return input
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/**
 * Derive a provider name from Base URL + Kind.
 * Format: `{domain}_{kindShort}` where domain is the second-to-last
 * hostname segment (e.g. "deepseek" from "api.deepseek.com") and
 * kindShort strips the `_compatible` suffix.
 */
export function deriveProviderName(url: string, kind: string): string {
  const host = new URL(url).hostname;
  const parts = host.split(".");
  const domain = parts[parts.length - 2] || host;
  const kindShort = kind.replace("_compatible", "");
  return `${domain}_${kindShort}`;
}

/**
 * Derive a provider name, appending `_2`, `_3`, ... on collision with
 * `existingNames` until unique.
 */
export function deriveUniqueProviderName(
  url: string,
  kind: string,
  existingNames: Set<string>,
): string {
  const base = deriveProviderName(url, kind);
  if (!existingNames.has(base)) return base;
  let i = 2;
  while (existingNames.has(`${base}_${i}`)) i++;
  return `${base}_${i}`;
}

/**
 * Validate a Base URL. Returns an error message string, or null if valid.
 * Checks: non-empty, starts with http:// or https://, parses as URL.
 */
export function validateBaseUrl(input: string): string | null {
  const trimmed = input.trim();
  if (!trimmed) return "Base URL 不能为空";
  if (!/^https?:\/\//.test(trimmed)) {
    return "请输入完整的 URL（以 http:// 或 https:// 开头）";
  }
  try {
    new URL(trimmed);
  } catch {
    return "URL 格式不正确";
  }
  return null;
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `pnpm --filter @busytok/gui test -- --run providerFormUtils`
Expected: PASS — all 14 tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/gui/src/pages/providerFormUtils.ts apps/gui/src/pages/providerFormUtils.test.ts
git commit -m "feat(gui): add providerFormUtils (parseTags, deriveProviderName, validateBaseUrl)

Pure helpers extracted ahead of the ProvidersPage rewrite. Per spec §3.
The CLI gets a Rust mirror in a later task."
```

---

## Task 4: Build ProviderCard Component (View Mode + Inline Models)

**Files:**
- Create: `apps/gui/src/components/ProviderCard.tsx`
- Test: `apps/gui/src/components/ProviderCard.test.tsx`

**Interfaces:**
- Consumes:
  - `ProviderDto` from `@busytok/protocol-types`
  - `ModelCatalogEntryDto[]` (filtered to this provider) — passed in as prop, NOT fetched here (parent does the single `useModels` query and groups)
  - Mutations from `useProviderMutations()` and `useModelMutations()` — passed in as props to keep the component testable in isolation
  - `reportFrontendEventSafely` from `../logging/safeReporter`
- Produces:
  - `ProviderCard` React component with this prop shape:
    ```typescript
    interface ProviderCardProps {
      provider: ProviderDto;
      models: ModelCatalogEntryDto[];
      isModelsLoading: boolean;
      providerMutations: ReturnType<typeof useProviderMutations>;
      modelMutations: ReturnType<typeof useModelMutations>;
      onEdit: () => void;          // parent enters edit mode (handled in Task 7)
      onTestConnection: (id: string) => void;
      onDelete: (provider: ProviderDto) => void;
    }
    ```

- [ ] **Step 1: Write the failing tests**

Create `apps/gui/src/components/ProviderCard.test.tsx`:

```typescript
import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import type { ProviderDto, ModelCatalogEntryDto } from "@busytok/protocol-types";
import { ProviderCard } from "./ProviderCard";

const makeProvider = (overrides: Partial<ProviderDto> = {}): ProviderDto => ({
  id: "prov-1",
  name: "deepseek_openai",
  provider_kind: "openai_compatible" as never,
  base_url: "https://api.deepseek.com/v1",
  enabled: true,
  has_api_key: true,
  created_at_ms: 0,
  updated_at_ms: 0,
  ...overrides,
});

const makeModel = (overrides: Partial<ModelCatalogEntryDto> = {}): ModelCatalogEntryDto => ({
  provider_id: "prov-1",
  provider_name: "deepseek_openai",
  provider_kind: "openai_compatible" as never,
  provider_enabled: true,
  model_db_id: "model-db-1",
  model_id: "deepseek-chat",
  model_enabled: true,
  tags: ["cheap", "fast"],
  display_name: "deepseek-chat",
  reasoning: false,
  context_window: 200000,
  max_tokens: 8192,
  ...overrides,
});

const noopMutations = {
  createProvider: { mutate: vi.fn(), isPending: false },
  updateProvider: { mutate: vi.fn(), isPending: false },
  deleteProvider: { mutate: vi.fn(), isPending: false },
  testConnection: { mutate: vi.fn(), isPending: false },
} as never;

const noopModelMutations = {
  createModel: { mutate: vi.fn(), isPending: false },
  updateModel: { mutate: vi.fn(), isPending: false },
  deleteModel: { mutate: vi.fn(), isPending: false },
  tagsUpdate: { mutate: vi.fn(), isPending: false },
} as never;

describe("ProviderCard (view mode)", () => {
  it("renders provider name, kind chip, and base url", () => {
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    expect(screen.getByText("deepseek_openai")).toBeInTheDocument();
    expect(screen.getByText("openai")).toBeInTheDocument();
    expect(screen.getByText("https://api.deepseek.com/v1")).toBeInTheDocument();
  });

  it("renders provider id in monospace", () => {
    render(
      <ProviderCard
        provider={makeProvider({ id: "abc-123" })}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    expect(screen.getByText(/abc-123/)).toBeInTheDocument();
  });

  it("renders model rows when models are provided", () => {
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[makeModel({ model_id: "deepseek-chat" }), makeModel({ model_id: "deepseek-reason" })]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    expect(screen.getByText("deepseek-chat")).toBeInTheDocument();
    expect(screen.getByText("deepseek-reason")).toBeInTheDocument();
  });

  it("renders tags as chips", () => {
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[makeModel({ tags: ["cheap", "fast"] })]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    expect(screen.getByText("cheap")).toBeInTheDocument();
    expect(screen.getByText("fast")).toBeInTheDocument();
  });

  it("renders empty-state message when models list is empty and not loading", () => {
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    expect(screen.getByText(/暂无 model/)).toBeInTheDocument();
  });

  it("renders loading state when isModelsLoading is true", () => {
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={true}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    expect(screen.getByText(/加载中/)).toBeInTheDocument();
  });

  it("calls onEdit when Edit button clicked", () => {
    const onEdit = vi.fn();
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={onEdit}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /编辑/i }));
    expect(onEdit).toHaveBeenCalledOnce();
  });

  it("calls onTestConnection when Test button clicked", () => {
    const onTestConnection = vi.fn();
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={onTestConnection}
        onDelete={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /测试连接/i }));
    expect(onTestConnection).toHaveBeenCalledWith("prov-1");
  });

  it("calls onDelete when Delete button clicked and user confirms", () => {
    const confirmSpy = vi.spyOn(globalThis, "confirm").mockReturnValue(true);
    const onDelete = vi.fn();
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={onDelete}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /删除/i }));
    expect(confirmSpy).toHaveBeenCalled();
    expect(onDelete).toHaveBeenCalledWith(makeProvider());
    confirmSpy.mockRestore();
  });

  it("does not call onDelete when user cancels confirm", () => {
    const confirmSpy = vi.spyOn(globalThis, "confirm").mockReturnValue(false);
    const onDelete = vi.fn();
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={onDelete}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /删除/i }));
    expect(onDelete).not.toHaveBeenCalled();
    confirmSpy.mockRestore();
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --filter @busytok/gui test -- --run ProviderCard`
Expected: FAIL with "Cannot find module './ProviderCard'".

- [ ] **Step 3: Write the implementation (view mode only — edit mode comes in Task 7)**

Create `apps/gui/src/components/ProviderCard.tsx`:

```typescript
import type {
  ModelCatalogEntryDto,
  ProviderDto,
} from "@busytok/protocol-types";
import type { useProviderMutations, useModelMutations } from "../api/useBusytokData";

interface ProviderCardProps {
  provider: ProviderDto;
  models: ModelCatalogEntryDto[];
  isModelsLoading: boolean;
  providerMutations: ReturnType<typeof useProviderMutations>;
  modelMutations: ReturnType<typeof useModelMutations>;
  onEdit: () => void;
  onTestConnection: (id: string) => void;
  onDelete: (provider: ProviderDto) => void;
}

const KIND_LABEL: Record<string, string> = {
  openai_compatible: "openai",
  anthropic_compatible: "anthropic",
};

export function ProviderCard({
  provider,
  models,
  isModelsLoading,
  onEdit,
  onTestConnection,
  onDelete,
}: ProviderCardProps) {
  const handleDelete = () => {
    const ok = globalThis.confirm(
      "确定删除此 provider 及其关联的所有 models？\n注意：已绑定此 provider/model 的 subagents 将在下次 delegate 时失败，需要手动重新绑定。",
    );
    if (ok) onDelete(provider);
  };

  return (
    <div className="provider-card">
      <div className="provider-card__header">
        <span className="provider-card__name">{provider.name}</span>
        <span className="chip chip--kind">{KIND_LABEL[provider.provider_kind] ?? provider.provider_kind}</span>
        <span>{provider.enabled ? "● enabled" : "○ disabled"}</span>
        <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
          <button type="button" onClick={onEdit}>编辑</button>
          <button type="button" onClick={() => onTestConnection(provider.id)}>测试连接</button>
          <button type="button" onClick={handleDelete}>删除</button>
        </div>
      </div>
      <div className="provider-card__info">
        <div>{provider.base_url}</div>
        <div style={{ fontFamily: "monospace" }}>ID: {provider.id}</div>
      </div>
      <div className="provider-card__models">
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
          <strong>Models</strong>
          <button type="button">+ Add Model</button>
        </div>
        {isModelsLoading ? (
          <div>加载中…</div>
        ) : models.length === 0 ? (
          <div style={{ color: "var(--color-text-muted)", fontSize: "0.85rem" }}>暂无 model</div>
        ) : (
          models.map((m) => (
            <div key={m.model_db_id} className="model-row">
              <span className="model-row__name">{m.model_id}</span>
              <span>{m.model_enabled ? "●enabled" : "○disabled"}</span>
              {m.tags.length > 0 && (
                <span className="model-row__tags">
                  {m.tags.map((t) => (
                    <span key={t} className="chip">{t}</span>
                  ))}
                </span>
              )}
              <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
                <button type="button">编辑</button>
                <button type="button">删除</button>
              </div>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `pnpm --filter @busytok/gui test -- --run ProviderCard`
Expected: PASS — all 10 tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/gui/src/components/ProviderCard.tsx apps/gui/src/components/ProviderCard.test.tsx
git commit -m "feat(gui): add ProviderCard component (view mode + inline model list)

Always-expanded card with header (name, kind chip, enabled, actions),
info row (base_url + id), and inline model rows with tag chips. Per
spec §4. Edit mode comes in a later task."
```

---

## Task 5: Add Model Inline Create/Edit/Delete to ProviderCard

**Files:**
- Modify: `apps/gui/src/components/ProviderCard.tsx`
- Modify: `apps/gui/src/components/ProviderCard.test.tsx`

**Interfaces:**
- Consumes: `parseTags` from `./providerFormUtils` (re-exported through the page directory? No — import via `../pages/providerFormUtils`)
- Produces: `ProviderCard` extended with:
  - `onModelCreate(payload: ModelCreateRequestDto) => void`
  - `onModelUpdate(model: ModelCatalogEntryDto, patch: ModelUpdateRequestDto) => void`
  - `onModelTagsUpdate(modelId: string, tags: string[]) => void`
  - `onModelDelete(model: ModelCatalogEntryDto) => void`

  (These are passed through from the parent's `useModelMutations` so the card stays testable.)

- [ ] **Step 1: Write the failing tests for model create**

Append to `apps/gui/src/components/ProviderCard.test.tsx`:

```typescript
// Add this import at the top of the file (merging with existing imports):
//   import type { ModelCreateRequestDto, ModelUpdateRequestDto } from "@busytok/protocol-types";
// Then append this describe block below the existing tests from Task 4:

describe("ProviderCard model create", () => {
  it("shows inline create form when + Add Model clicked", () => {
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    expect(screen.getByPlaceholderText(/model name/i)).toBeInTheDocument();
  });

  it("calls onModelCreate with derived payload on Save", () => {
    const onModelCreate = vi.fn();
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={onModelCreate}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    const expected: ModelCreateRequestDto = {
      provider_id: "prov-1",
      model_id: "deepseek-chat",
      display_name: "deepseek-chat",
      context_window: 200000,
      max_tokens: 8192,
      reasoning: true,
      enabled: true,
      tags: [],
    };
    expect(onModelCreate).toHaveBeenCalledWith(expected);
  });

  it("parses tags from comma-separated input", () => {
    const onModelCreate = vi.fn();
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={onModelCreate}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.change(screen.getByPlaceholderText(/tags/i), { target: { value: "cheap, fast" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(onModelCreate).toHaveBeenCalledWith(
      expect.objectContaining({ tags: ["cheap", "fast"] }),
    );
  });
});

describe("ProviderCard model delete", () => {
  it("calls onModelDelete after confirm", () => {
    const confirmSpy = vi.spyOn(globalThis, "confirm").mockReturnValue(true);
    const onModelDelete = vi.fn();
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[makeModel()]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={onModelDelete}
      />,
    );
    // The first "删除" button in the model row (not the provider delete).
    const deleteButtons = screen.getAllByRole("button", { name: /删除/i });
    // Provider delete is the third button in the header; model delete is the second button in the row.
    // Use getAllByRole and pick the model-row one (index 4 = first model row's delete).
    fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    expect(onModelDelete).toHaveBeenCalledWith(makeModel());
    confirmSpy.mockRestore();
  });
});

describe("ProviderCard model edit", () => {
  const renderCardWithModel = (overrides: { onModelUpdate?: ReturnType<typeof vi.fn>; onModelTagsUpdate?: ReturnType<typeof vi.fn> } = {}) => {
    const onModelUpdate = overrides.onModelUpdate ?? vi.fn();
    const onModelTagsUpdate = overrides.onModelTagsUpdate ?? vi.fn();
    const result = render(
      <ProviderCard
        provider={makeProvider()}
        models={[makeModel()]}
        isModelsLoading={false}
        providerMutations={noopMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={onModelUpdate}
        onModelTagsUpdate={onModelTagsUpdate}
        onModelDelete={vi.fn()}
      />,
    );
    return { ...result, onModelUpdate, onModelTagsUpdate };
  };

  it("shows edit form with current model values when 编辑 clicked", () => {
    renderCardWithModel();
    // Click the model-row 编辑 button (not any provider-level edit).
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    // Edit form should show current display_name, tags, context_window, max_tokens, reasoning, enabled.
    expect(screen.getByDisplayValue("deepseek-chat")).toBeInTheDocument(); // display_name
    expect(screen.getByDisplayValue("200000")).toBeInTheDocument(); // context_window
    expect(screen.getByDisplayValue("8192")).toBeInTheDocument(); // max_tokens
    expect(screen.getByRole("checkbox", { name: /reasoning/i })).not.toBeChecked(); // makeModel has reasoning: false
  });

  it("calls onModelUpdate with only changed fields on save (single-state Option semantics)", () => {
    const { onModelUpdate } = renderCardWithModel();
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    // Change only display_name; leave other fields unchanged.
    const nameInput = screen.getByDisplayValue("deepseek-chat");
    fireEvent.change(nameInput, { target: { value: "DeepSeek Chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    // First arg is the model being edited; second is the patch with id + only changed field.
    expect(onModelUpdate).toHaveBeenCalledWith(
      makeModel(),
      expect.objectContaining({
        id: "model-db-1",
        display_name: "DeepSeek Chat",
      }),
    );
    // Unchanged fields must NOT be in the patch (omit = no change per ModelUpdateRequestDto semantics).
    const call = onModelUpdate.mock.calls[0][1];
    expect(call.context_window).toBeUndefined();
    expect(call.max_tokens).toBeUndefined();
    expect(call.reasoning).toBeUndefined();
    expect(call.enabled).toBeUndefined();
  });

  it("calls onModelTagsUpdate when tags changed on save", () => {
    const { onModelTagsUpdate } = renderCardWithModel();
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    // Change the tags input (empty in makeModel → set to "cheap,fast").
    const tagsInput = screen.getByPlaceholderText(/tags/i);
    fireEvent.change(tagsInput, { target: { value: "cheap,fast" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(onModelTagsUpdate).toHaveBeenCalledWith("deepseek-chat", ["cheap", "fast"]);
  });

  it("does not call onModelTagsUpdate when tags unchanged", () => {
    const { onModelTagsUpdate } = renderCardWithModel({
      onModelTagsUpdate: vi.fn(),
    });
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    // Don't touch tags — leave them as-is (empty in makeModel).
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(onModelTagsUpdate).not.toHaveBeenCalled();
  });

  it("exits edit mode on Cancel", () => {
    renderCardWithModel();
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    fireEvent.click(screen.getByRole("button", { name: /取消/i }));
    // Edit form should be gone; the model row's view mode should show again.
    expect(screen.queryByRole("checkbox", { name: /reasoning/i })).not.toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --filter @busytok/gui test -- --run ProviderCard`
Expected: FAIL — new tests fail because `onModelCreate` etc. are not yet wired.

- [ ] **Step 3: Extend ProviderCard with model create/edit/delete**

Edit `apps/gui/src/components/ProviderCard.tsx`. Replace the file's content with:

```typescript
import { useState } from "react";
import type {
  ModelCatalogEntryDto,
  ModelCreateRequestDto,
  ModelUpdateRequestDto,
  ProviderDto,
} from "@busytok/protocol-types";
import type { useProviderMutations, useModelMutations } from "../api/useBusytokData";
import { parseTags } from "../pages/providerFormUtils";

interface ProviderCardProps {
  provider: ProviderDto;
  models: ModelCatalogEntryDto[];
  isModelsLoading: boolean;
  providerMutations: ReturnType<typeof useProviderMutations>;
  modelMutations: ReturnType<typeof useModelMutations>;
  onEdit: () => void;
  onTestConnection: (id: string) => void;
  onDelete: (provider: ProviderDto) => void;
  onModelCreate: (payload: ModelCreateRequestDto) => void;
  onModelUpdate: (model: ModelCatalogEntryDto, patch: ModelUpdateRequestDto) => void;
  onModelTagsUpdate: (modelId: string, tags: string[]) => void;
  onModelDelete: (model: ModelCatalogEntryDto) => void;
}

const KIND_LABEL: Record<string, string> = {
  openai_compatible: "openai",
  anthropic_compatible: "anthropic",
};

interface NewModelDraft {
  modelId: string;
  tags: string;
}

interface ModelEditDraft {
  display_name: string;
  tags: string;
  context_window: number;
  max_tokens: number;
  reasoning: boolean;
  enabled: boolean;
}

function toEditDraft(m: ModelCatalogEntryDto): ModelEditDraft {
  return {
    display_name: m.display_name ?? "",
    tags: m.tags.join(", "),
    context_window: m.context_window ?? 200000,
    max_tokens: m.max_tokens ?? 8192,
    reasoning: m.reasoning ?? false,
    enabled: m.model_enabled,
  };
}

export function ProviderCard({
  provider,
  models,
  isModelsLoading,
  onEdit,
  onTestConnection,
  onDelete,
  onModelCreate,
  onModelUpdate,
  onModelTagsUpdate,
  onModelDelete,
}: ProviderCardProps) {
  const [showCreateModel, setShowCreateModel] = useState(false);
  const [newModelDraft, setNewModelDraft] = useState<NewModelDraft>({ modelId: "", tags: "" });
  const [editingModelDbId, setEditingModelDbId] = useState<string | null>(null);
  const [editDraft, setEditDraft] = useState<ModelEditDraft | null>(null);

  const handleProviderDelete = () => {
    const ok = globalThis.confirm(
      "确定删除此 provider 及其关联的所有 models？\n注意：已绑定此 provider/model 的 subagents 将在下次 delegate 时失败，需要手动重新绑定。",
    );
    if (ok) onDelete(provider);
  };

  const handleModelDelete = (model: ModelCatalogEntryDto) => {
    const ok = globalThis.confirm(
      "确定删除此 model？\n注意：已绑定此 model 的 subagents 将在下次 delegate 时失败。",
    );
    if (ok) onModelDelete(model);
  };

  const handleCreateSubmit = () => {
    if (!newModelDraft.modelId.trim()) return;
    const payload: ModelCreateRequestDto = {
      provider_id: provider.id,
      model_id: newModelDraft.modelId.trim(),
      display_name: newModelDraft.modelId.trim(),
      context_window: 200000,
      max_tokens: 8192,
      reasoning: true,
      enabled: true,
      tags: parseTags(newModelDraft.tags),
    };
    onModelCreate(payload);
    setNewModelDraft({ modelId: "", tags: "" });
    setShowCreateModel(false);
  };

  const startModelEdit = (m: ModelCatalogEntryDto) => {
    setEditingModelDbId(m.model_db_id);
    setEditDraft(toEditDraft(m));
  };

  const cancelModelEdit = () => {
    setEditingModelDbId(null);
    setEditDraft(null);
  };

  const handleEditSubmit = (m: ModelCatalogEntryDto) => {
    if (!editDraft) return;
    const patch: ModelUpdateRequestDto = { id: m.model_db_id };
    // Single-state Option<T>: only include fields that changed.
    if (editDraft.display_name !== (m.display_name ?? "")) {
      patch.display_name = editDraft.display_name;
    }
    if (editDraft.context_window !== (m.context_window ?? 200000)) {
      patch.context_window = editDraft.context_window;
    }
    if (editDraft.max_tokens !== (m.max_tokens ?? 8192)) {
      patch.max_tokens = editDraft.max_tokens;
    }
    if (editDraft.reasoning !== (m.reasoning ?? false)) {
      patch.reasoning = editDraft.reasoning;
    }
    if (editDraft.enabled !== m.model_enabled) {
      patch.enabled = editDraft.enabled;
    }
    if (Object.keys(patch).length > 1) {
      // More than just `id` → there are field updates.
      onModelUpdate(m, patch);
    }
    // Tags are updated via a separate RPC. Compare parsed arrays to avoid
    // false positives from whitespace differences in the comma-separated string.
    const newTags = parseTags(editDraft.tags);
    const oldTagsSameOrder = m.tags.length === newTags.length && m.tags.every((t, i) => t === newTags[i]);
    if (!oldTagsSameOrder) {
      onModelTagsUpdate(m.model_id, newTags);
    }
    cancelModelEdit();
  };

  return (
    <div className="provider-card">
      <div className="provider-card__header">
        <span className="provider-card__name">{provider.name}</span>
        <span className="chip chip--kind">{KIND_LABEL[provider.provider_kind] ?? provider.provider_kind}</span>
        <span>{provider.enabled ? "● enabled" : "○ disabled"}</span>
        <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
          <button type="button" onClick={onEdit}>编辑</button>
          <button type="button" onClick={() => onTestConnection(provider.id)}>测试连接</button>
          <button type="button" onClick={handleProviderDelete}>删除</button>
        </div>
      </div>
      <div className="provider-card__info">
        <div>{provider.base_url}</div>
        <div style={{ fontFamily: "monospace" }}>ID: {provider.id}</div>
      </div>
      <div className="provider-card__models">
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
          <strong>Models</strong>
          <button type="button" onClick={() => setShowCreateModel((v) => !v)}>+ Add Model</button>
        </div>
        {showCreateModel && (
          <div className="model-row" style={{ flexDirection: "column", alignItems: "stretch", gap: 6 }}>
            <input
              type="text"
              placeholder="model name (e.g. deepseek-chat)"
              value={newModelDraft.modelId}
              onChange={(e) => setNewModelDraft((d) => ({ ...d, modelId: e.target.value }))}
            />
            <input
              type="text"
              placeholder="tags (comma-separated, optional)"
              value={newModelDraft.tags}
              onChange={(e) => setNewModelDraft((d) => ({ ...d, tags: e.target.value }))}
            />
            <div style={{ display: "flex", gap: 8 }}>
              <button type="button" onClick={handleCreateSubmit}>保存</button>
              <button type="button" onClick={() => { setShowCreateModel(false); setNewModelDraft({ modelId: "", tags: "" }); }}>取消</button>
            </div>
          </div>
        )}
        {isModelsLoading ? (
          <div>加载中…</div>
        ) : models.length === 0 && !showCreateModel ? (
          <div style={{ color: "var(--color-text-muted)", fontSize: "0.85rem" }}>暂无 model</div>
        ) : (
          models.map((m) => (
            <div key={m.model_db_id} className="model-row">
              {editingModelDbId === m.model_db_id && editDraft ? (
                <div style={{ width: "100%", flexDirection: "column", alignItems: "stretch", gap: 6, display: "flex" }}>
                  <label>
                    Display Name
                    <input
                      type="text"
                      value={editDraft.display_name}
                      onChange={(e) => setEditDraft({ ...editDraft, display_name: e.target.value })}
                    />
                  </label>
                  <label>
                    Tags
                    <input
                      type="text"
                      placeholder="tags (comma-separated)"
                      value={editDraft.tags}
                      onChange={(e) => setEditDraft({ ...editDraft, tags: e.target.value })}
                    />
                  </label>
                  <label>
                    Context Window
                    <input
                      type="number"
                      value={editDraft.context_window}
                      onChange={(e) => setEditDraft({ ...editDraft, context_window: Number(e.target.value) })}
                    />
                  </label>
                  <label>
                    Max Tokens
                    <input
                      type="number"
                      value={editDraft.max_tokens}
                      onChange={(e) => setEditDraft({ ...editDraft, max_tokens: Number(e.target.value) })}
                    />
                  </label>
                  <label>
                    <input
                      type="checkbox"
                      checked={editDraft.reasoning}
                      onChange={(e) => setEditDraft({ ...editDraft, reasoning: e.target.checked })}
                    />
                    Reasoning
                  </label>
                  <label>
                    <input
                      type="checkbox"
                      checked={editDraft.enabled}
                      onChange={(e) => setEditDraft({ ...editDraft, enabled: e.target.checked })}
                    />
                    Enabled
                  </label>
                  <div style={{ display: "flex", gap: 8 }}>
                    <button type="button" onClick={() => handleEditSubmit(m)}>保存</button>
                    <button type="button" onClick={cancelModelEdit}>取消</button>
                  </div>
                </div>
              ) : (
                <>
                  <span className="model-row__name">{m.model_id}</span>
                  <span>{m.model_enabled ? "●enabled" : "○disabled"}</span>
                  {m.tags.length > 0 && (
                    <span className="model-row__tags">
                      {m.tags.map((t) => (
                        <span key={t} className="chip">{t}</span>
                      ))}
                    </span>
                  )}
                  <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
                    <button type="button" onClick={() => startModelEdit(m)}>编辑</button>
                    <button type="button" onClick={() => handleModelDelete(m)}>删除</button>
                  </div>
                </>
              )}
            </div>
          ))
        )}
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `pnpm --filter @busytok/gui test -- --run ProviderCard`
Expected: PASS — all tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/gui/src/components/ProviderCard.tsx apps/gui/src/components/ProviderCard.test.tsx
git commit -m "feat(gui): add inline model create/edit/delete to ProviderCard

+ Add Model toggles an inline form (model name + tags, defaults
context_window=200000, max_tokens=8192, reasoning=true, enabled=true,
display_name=model_id). Edit expands a model row with Display Name,
Tags, Context Window, Max Tokens, Reasoning, Enabled fields; Save calls
onModelUpdate with only changed fields (single-state Option semantics)
and onModelTagsUpdate when tags differ. Delete confirms with
dangling-subagent warning. Per spec §5."
```

---

## Task 6: Build New ProviderCreationForm with Partial-Success State

**Files:**
- Create: `apps/gui/src/components/ProviderCreationForm.tsx`
- Test: `apps/gui/src/components/ProviderCreationForm.test.tsx`

**Interfaces:**
- Consumes:
  - `useProviders()` — to build the `existingNames` set for collision check
  - `useProviderMutations().createProvider` and `useModelMutations().createModel`
  - `parseTags`, `deriveUniqueProviderName`, `validateBaseUrl` from `../pages/providerFormUtils`
  - `reportFrontendEventSafely` from `../logging/safeReporter`
- Produces: `ProviderCreationForm` component with this prop shape:
  ```typescript
  interface ProviderCreationFormProps {
    onClose: () => void;
  }
  ```

  The form drives `useProviderMutations().createProvider` + `useModelMutations().createModel` directly; no `onProviderCreated` callback is needed because the page observes changes via the invalidated `useProviders()` query (TanStack Query refetch).

  Internal state machine: `idle` → `creating` → (`success` | `partial-success` | `provider-failed`).

- [ ] **Step 1: Write the failing tests**

Create `apps/gui/src/components/ProviderCreationForm.test.tsx`:

```typescript
import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ProviderDto, ProviderListResponseDto } from "@busytok/protocol-types";
import { ProviderCreationForm } from "./ProviderCreationForm";

vi.mock("../api/useBusytokData", () => ({
  useProviders: vi.fn(),
  useProviderMutations: vi.fn(),
  useModelMutations: vi.fn(),
}));
vi.mock("../logging/safeReporter", () => ({
  reportFrontendEventSafely: vi.fn(),
}));

import { useProviders, useProviderMutations, useModelMutations } from "../api/useBusytokData";
import { reportFrontendEventSafely } from "../logging/safeReporter";

const makeProvider = (overrides: Partial<ProviderDto> = {}): ProviderDto => ({
  id: "prov-new",
  name: "deepseek_openai",
  provider_kind: "openai_compatible" as never,
  base_url: "https://api.deepseek.com/v1",
  enabled: true,
  has_api_key: true,
  created_at_ms: 0,
  updated_at_ms: 0,
  ...overrides,
});

function renderForm(overrides: { existingNames?: string[]; createProvider?: any; createModel?: any } = {}) {
  const mockUseProviders = vi.mocked(useProviders);
  mockUseProviders.mockReturnValue({
    data: {
      providers: (overrides.existingNames ?? []).map((n) => makeProvider({ name: n })),
    } as ProviderListResponseDto,
    isLoading: false,
    isError: false,
    isFetching: false,
  } as never);

  const createProvider = overrides.createProvider ?? vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
    opts?.onSuccess?.(makeProvider());
  });
  const createModel = overrides.createModel ?? vi.fn((_payload: unknown, opts?: { onSuccess?: () => void; onError?: (e: Error) => void }) => {
    opts?.onSuccess?.();
  });

  vi.mocked(useProviderMutations).mockReturnValue({
    createProvider,
    updateProvider: { mutate: vi.fn(), isPending: false },
    deleteProvider: { mutate: vi.fn(), isPending: false },
    testConnection: { mutate: vi.fn(), isPending: false },
  } as never);
  vi.mocked(useModelMutations).mockReturnValue({
    createModel,
    updateModel: { mutate: vi.fn(), isPending: false },
    deleteModel: { mutate: vi.fn(), isPending: false },
    tagsUpdate: { mutate: vi.fn(), isPending: false },
  } as never);

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ProviderCreationForm onClose={vi.fn()} />
    </QueryClientProvider>,
  );
}

function fillForm() {
  fireEvent.change(screen.getByPlaceholderText(/base url/i), { target: { value: "https://api.deepseek.com/v1" } });
  fireEvent.change(screen.getByPlaceholderText(/api key/i), { target: { value: "sk-test" } });
}

describe("ProviderCreationForm", () => {
  it("validates base URL on blur", () => {
    renderForm();
    const urlInput = screen.getByPlaceholderText(/base url/i);
    fireEvent.change(urlInput, { target: { value: "bad-url" } });
    fireEvent.blur(urlInput);
    expect(screen.getByText(/请输入完整的 URL/i)).toBeInTheDocument();
  });

  it("disables Save when API key is empty", () => {
    renderForm();
    fireEvent.change(screen.getByPlaceholderText(/base url/i), { target: { value: "https://api.deepseek.com/v1" } });
    expect(screen.getByRole("button", { name: /^保存$/i })).toBeDisabled();
  });

  it("calls createProvider with derived name on Save (no model)", () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
    renderForm({ createProvider });
    fillForm();
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(createProvider).toHaveBeenCalledWith(
      expect.objectContaining({
        name: "deepseek_openai",
        provider_kind: "openai_compatible",
        base_url: "https://api.deepseek.com/v1",
        api_key: "sk-test",
        enabled: true,
      }),
      expect.anything(),
    );
  });

  it("derives name with _2 suffix on collision", () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider({ name: "deepseek_openai_2" }));
    });
    renderForm({ existingNames: ["deepseek_openai"], createProvider });
    fillForm();
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(createProvider).toHaveBeenCalledWith(
      expect.objectContaining({ name: "deepseek_openai_2" }),
      expect.anything(),
    );
  });

  it("calls createModel after createProvider when model name is filled", () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
    const createModel = vi.fn((_payload: unknown, opts?: { onSuccess?: () => void }) => {
      opts?.onSuccess?.();
    });
    renderForm({ createProvider, createModel });
    fillForm();
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(createModel).toHaveBeenCalledWith(
      expect.objectContaining({
        provider_id: "prov-new",
        model_id: "deepseek-chat",
        display_name: "deepseek-chat",
        context_window: 200000,
        max_tokens: 8192,
        reasoning: true,
        enabled: true,
        tags: [],
      }),
      expect.anything(),
    );
  });

  it("enters partial-success state when createModel fails after createProvider succeeds", async () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
    const createModel = vi.fn((_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
      opts?.onError?.(new Error("model already exists"));
    });
    renderForm({ createProvider, createModel });
    fillForm();
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));

    await waitFor(() => {
      expect(screen.getByText(/model already exists/i)).toBeInTheDocument();
    });
    // Save button should be disabled to prevent duplicate provider creation
    expect(screen.getByRole("button", { name: /^保存$/i })).toBeDisabled();
    // Retry button should be enabled
    expect(screen.getByRole("button", { name: /重试 model/i })).toBeEnabled();
  });

  it("emits provider.added and model.add.failed events on partial success", async () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
    const createModel = vi.fn((_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
      opts?.onError?.(new Error("model already exists"));
    });
    renderForm({ createProvider, createModel });
    fillForm();
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));

    await waitFor(() => {
      expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
        expect.objectContaining({ event_code: "provider.added" }),
      );
    });
    await waitFor(() => {
      expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
        expect.objectContaining({ event_code: "model.add.failed" }),
      );
    });
  });

  it("retries only createModel (not createProvider) on partial-success retry", async () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onSuccess?: (p: ProviderDto) => void }) => {
      opts?.onSuccess?.(makeProvider());
    });
    const createModel = vi.fn((_payload: unknown, opts?: { onError?: (e: Error) => void; onSuccess?: () => void }) => {
      // First call fails, second succeeds
      if ((createModel as any).mock.calls.length === 1) {
        opts?.onError?.(new Error("model already exists"));
      } else {
        opts?.onSuccess?.();
      }
    });
    renderForm({ createProvider, createModel });
    fillForm();
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "deepseek-chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));

    await waitFor(() => expect(screen.getByRole("button", { name: /重试 model/i })).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: /重试 model/i }));

    // createProvider should only have been called once
    expect(createProvider).toHaveBeenCalledTimes(1);
    // createModel should have been called twice
    await waitFor(() => expect(createModel).toHaveBeenCalledTimes(2));
  });

  it("emits provider.add.failed when createProvider fails", async () => {
    const createProvider = vi.fn((_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
      opts?.onError?.(new Error("network error"));
    });
    renderForm({ createProvider });
    fillForm();
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));

    await waitFor(() => {
      expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
        expect.objectContaining({ event_code: "provider.add.failed" }),
      );
    });
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --filter @busytok/gui test -- --run ProviderCreationForm`
Expected: FAIL with "Cannot find module './ProviderCreationForm'".

- [ ] **Step 3: Write the implementation**

Create `apps/gui/src/components/ProviderCreationForm.tsx`:

```typescript
import { useMemo, useState } from "react";
import type {
  ModelCreateRequestDto,
  ProviderCreateRequestDto,
  ProviderDto,
} from "@busytok/protocol-types";
import { useModelMutations, useProviderMutations, useProviders } from "../api/useBusytokData";
import { reportFrontendEventSafely } from "../logging/safeReporter";
import {
  deriveUniqueProviderName,
  parseTags,
  validateBaseUrl,
} from "../pages/providerFormUtils";

interface ProviderCreationFormProps {
  onClose: () => void;
}

type SubmitState =
  | { kind: "idle" }
  | { kind: "provider-creating" }
  | { kind: "partial-success"; provider: ProviderDto; modelError: string }
  | { kind: "provider-failed"; error: string };

export function ProviderCreationForm({ onClose }: ProviderCreationFormProps) {
  const providersQuery = useProviders();
  const { createProvider } = useProviderMutations();
  const { createModel } = useModelMutations();

  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [kind, setKind] = useState<"openai_compatible" | "anthropic_compatible">("openai_compatible");
  const [modelName, setModelName] = useState("");
  const [modelTags, setModelTags] = useState("");
  const [urlError, setUrlError] = useState<string | null>(null);
  const [state, setState] = useState<SubmitState>({ kind: "idle" });

  const existingNames = useMemo(
    () => new Set((providersQuery.data?.providers ?? []).map((p) => p.name)),
    [providersQuery.data],
  );

  const canSubmit =
    validateBaseUrl(baseUrl) === null &&
    apiKey.trim().length > 0 &&
    (state.kind === "idle" || state.kind === "provider-failed");

  const handleBlurUrl = () => setUrlError(validateBaseUrl(baseUrl));

  const buildProviderPayload = (): ProviderCreateRequestDto => {
    const name = deriveUniqueProviderName(baseUrl, kind, existingNames);
    return {
      name,
      provider_kind: kind,
      base_url: baseUrl.trim(),
      api_key: apiKey,
      enabled: true,
    };
  };

  const buildModelPayload = (providerId: string): ModelCreateRequestDto => ({
    provider_id: providerId,
    model_id: modelName.trim(),
    display_name: modelName.trim(),
    context_window: 200000,
    max_tokens: 8192,
    reasoning: true,
    enabled: true,
    tags: parseTags(modelTags),
  });

  const handleProviderSuccess = (provider: ProviderDto) => {
    reportFrontendEventSafely({
      level: "INFO",
      event_code: "provider.added",
      message: "Provider added",
      details: { name: provider.name },
    });

    if (!modelName.trim()) {
      // No model to create — full success
      onClose();
      return;
    }

    // Try to create the model
    createModel(buildModelPayload(provider.id), {
      onSuccess: () => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "model.added",
          message: "Model added",
          details: { provider_id: provider.id, model_id: modelName.trim() },
        });
        onClose();
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "model.add.failed",
          message: "Model creation failed",
          details: { provider_id: provider.id, model_id: modelName.trim(), error: err.message },
        });
        setState({ kind: "partial-success", provider, modelError: err.message });
      },
    });
  };

  const handleProviderError = (err: Error) => {
    reportFrontendEventSafely({
      level: "ERROR",
      event_code: "provider.add.failed",
      message: "Provider creation failed",
      details: { error: err.message },
    });
    setState({ kind: "provider-failed", error: err.message });
  };

  const handleSubmit = () => {
    const urlErr = validateBaseUrl(baseUrl);
    if (urlErr) {
      setUrlError(urlErr);
      return;
    }
    if (!apiKey.trim()) return;

    setState({ kind: "provider-creating" });
    createProvider(buildProviderPayload(), {
      onSuccess: handleProviderSuccess,
      onError: handleProviderError,
    });
  };

  const handleRetryModel = () => {
    if (state.kind !== "partial-success") return;
    createModel(buildModelPayload(state.provider.id), {
      onSuccess: () => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "model.added",
          message: "Model added",
          details: { provider_id: state.provider.id, model_id: modelName.trim() },
        });
        onClose();
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "model.add.failed",
          message: "Model creation failed",
          details: { provider_id: state.provider.id, model_id: modelName.trim(), error: err.message },
        });
        setState({ kind: "partial-success", provider: state.provider, modelError: err.message });
      },
    });
  };

  return (
    <div className="provider-card">
      <div className="provider-card__header">
        <strong>新建 Provider</strong>
      </div>
      <div style={{ padding: 16, display: "flex", flexDirection: "column", gap: 12 }}>
        <label>
          Base URL
          <input
            type="text"
            placeholder="Base URL (https://...)"
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            onBlur={handleBlurUrl}
          />
        </label>
        {urlError && <div style={{ color: "var(--color-status-danger)", fontSize: "0.85rem" }}>{urlError}</div>}
        <label>
          API Key
          <input
            type="password"
            placeholder="API Key"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
          />
        </label>
        <label>
          Kind
          <select value={kind} onChange={(e) => setKind(e.target.value as typeof kind)}>
            <option value="openai_compatible">openai_compatible</option>
            <option value="anthropic_compatible">anthropic_compatible</option>
          </select>
        </label>
        <hr />
        <div>同步创建 Model</div>
        <label>
          Model Name
          <input
            type="text"
            placeholder="model name (optional)"
            value={modelName}
            onChange={(e) => setModelName(e.target.value)}
          />
        </label>
        <label>
          Model Tags
          <input
            type="text"
            placeholder="tags (comma-separated, optional)"
            value={modelTags}
            onChange={(e) => setModelTags(e.target.value)}
          />
        </label>

        {state.kind === "partial-success" && (
          <div className="provider-card__error-banner">
            Provider 已创建，但 Model 创建失败：{state.modelError}
          </div>
        )}
        {state.kind === "provider-failed" && (
          <div className="provider-card__error-banner">
            Provider 创建失败：{state.error}
          </div>
        )}

        <div style={{ display: "flex", gap: 8 }}>
          <button type="button" onClick={handleSubmit} disabled={!canSubmit}>
            保存
          </button>
          {state.kind === "partial-success" && (
            <button type="button" onClick={handleRetryModel}>重试 Model</button>
          )}
          <button type="button" onClick={onClose}>取消</button>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `pnpm --filter @busytok/gui test -- --run ProviderCreationForm`
Expected: PASS — all 9 tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/gui/src/components/ProviderCreationForm.tsx apps/gui/src/components/ProviderCreationForm.test.tsx
git commit -m "feat(gui): add ProviderCreationForm with partial-success state

Three submit states: idle, partial-success (provider created, model
failed — Save disabled, Retry Model enabled), provider-failed. Emits
provider.added / model.added / *.failed events per spec §8. URL
validation + name derivation + collision handling per spec §3."
```

---

## Task 7: Add ProviderCard Edit Mode (Inline Header Edit)

**Files:**
- Modify: `apps/gui/src/components/ProviderCard.tsx`
- Modify: `apps/gui/src/components/ProviderCard.test.tsx`

**Interfaces:**
- Consumes: `useProviderMutations().updateProvider` (already in props)
- Produces: `ProviderCard` extended with an `isEditing` prop and `onCancelEdit` / `onSaveEdit` callbacks. The parent (ProvidersPage) tracks which provider is being edited.

- [ ] **Step 1: Write the failing tests for edit mode**

Append to `apps/gui/src/components/ProviderCard.test.tsx`:

```typescript
describe("ProviderCard edit mode", () => {
  const renderCardInEditMode = (overrides: { updateProvider?: any } = {}) => {
    const updateProvider = overrides.updateProvider ?? vi.fn((_payload: unknown, opts?: { onSuccess?: () => void }) => {
      opts?.onSuccess?.();
    });
    const providerMutations = {
      ...noopMutations,
      updateProvider,
    } as never;
    render(
      <ProviderCard
        provider={makeProvider()}
        models={[makeModel()]}
        isModelsLoading={false}
        providerMutations={providerMutations}
        modelMutations={noopModelMutations}
        onEdit={vi.fn()}
        onTestConnection={vi.fn()}
        onDelete={vi.fn()}
        onModelCreate={vi.fn()}
        onModelUpdate={vi.fn()}
        onModelTagsUpdate={vi.fn()}
        onModelDelete={vi.fn()}
        isEditing={true}
        onCancelEdit={vi.fn()}
      />,
    );
    return { updateProvider };
  };

  it("renders editable inputs for base url, api key, kind, name when isEditing", () => {
    renderCardInEditMode();
    expect(screen.getByDisplayValue("https://api.deepseek.com/v1")).toBeInTheDocument();
    expect(screen.getByDisplayValue("deepseek_openai")).toBeInTheDocument();
    // API key field shows placeholder for new key
    expect(screen.getByPlaceholderText(/new api key/i)).toBeInTheDocument();
  });

  it("disables model operation buttons when editing", () => {
    renderCardInEditMode();
    const addModelButton = screen.getByRole("button", { name: /\+ Add Model/i });
    expect(addModelButton).toBeDisabled();
  });

  it("shows notice when editing", () => {
    renderCardInEditMode();
    expect(screen.getByText(/正在编辑 Provider 信息/i)).toBeInTheDocument();
  });

  it("calls updateProvider with patch on Save", () => {
    const { updateProvider } = renderCardInEditMode();
    fireEvent.change(screen.getByDisplayValue("https://api.deepseek.com/v1"), { target: { value: "https://api.deepseek.com/v2" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(updateProvider).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "prov-1",
        base_url: "https://api.deepseek.com/v2",
      }),
      expect.anything(),
    );
  });

  it("omits api_key from patch when key field is empty (no change)", () => {
    const { updateProvider } = renderCardInEditMode();
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    const call = (updateProvider as any).mock.calls[0][0];
    expect(call.api_key).toBeUndefined();
  });

  it("includes api_key in patch when key field is filled", () => {
    const { updateProvider } = renderCardInEditMode();
    fireEvent.change(screen.getByPlaceholderText(/new api key/i), { target: { value: "sk-new" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    const call = (updateProvider as any).mock.calls[0][0];
    expect(call.api_key).toBe("sk-new");
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --filter @busytok/gui test -- --run ProviderCard`
Expected: FAIL — edit mode tests fail because `isEditing` prop isn't implemented.

- [ ] **Step 3: Add edit mode to ProviderCard**

Edit `apps/gui/src/components/ProviderCard.tsx`. Replace its content with the version that adds an `isEditing` branch. Key additions:

```typescript
import { useEffect, useState } from "react";
import type {
  ModelCatalogEntryDto,
  ModelCreateRequestDto,
  ModelUpdateRequestDto,
  ProviderDto,
  ProviderKind,
  ProviderUpdateRequestDto,
} from "@busytok/protocol-types";
import type { useProviderMutations, useModelMutations } from "../api/useBusytokData";
import { parseTags } from "../pages/providerFormUtils";
import { reportFrontendEventSafely } from "../logging/safeReporter";

interface ProviderCardProps {
  provider: ProviderDto;
  models: ModelCatalogEntryDto[];
  isModelsLoading: boolean;
  providerMutations: ReturnType<typeof useProviderMutations>;
  modelMutations: ReturnType<typeof useModelMutations>;
  onEdit: () => void;
  onTestConnection: (id: string) => void;
  onDelete: (provider: ProviderDto) => void;
  onModelCreate: (payload: ModelCreateRequestDto) => void;
  onModelUpdate: (model: ModelCatalogEntryDto, patch: ModelUpdateRequestDto) => void;
  onModelTagsUpdate: (modelId: string, tags: string[]) => void;
  onModelDelete: (model: ModelCatalogEntryDto) => void;
  isEditing?: boolean;
  onCancelEdit?: () => void;
}

const KIND_LABEL: Record<string, string> = {
  openai_compatible: "openai",
  anthropic_compatible: "anthropic",
};

interface NewModelDraft {
  modelId: string;
  tags: string;
}

interface EditDraft {
  name: string;
  base_url: string;
  api_key: string;
  provider_kind: ProviderKind;
}

export function ProviderCard({
  provider,
  models,
  isModelsLoading,
  onEdit,
  onTestConnection,
  onDelete,
  onModelCreate,
  onModelDelete,
  isEditing = false,
  onCancelEdit,
  providerMutations,
}: ProviderCardProps) {
  const [showCreateModel, setShowCreateModel] = useState(false);
  const [newModelDraft, setNewModelDraft] = useState<NewModelDraft>({ modelId: "", tags: "" });
  const [editDraft, setEditDraft] = useState<EditDraft | null>(null);

  // Initialize/clear edit draft via useEffect — never setState during render
  // (React 19 + StrictMode render phase must be pure).
  useEffect(() => {
    if (isEditing) {
      setEditDraft({
        name: provider.name,
        base_url: provider.base_url,
        api_key: "",
        provider_kind: provider.provider_kind,
      });
    } else {
      setEditDraft(null);
    }
  }, [isEditing, provider.id, provider.name, provider.base_url, provider.provider_kind]);

  const handleProviderDelete = () => {
    const ok = globalThis.confirm(
      "确定删除此 provider 及其关联的所有 models？\n注意：已绑定此 provider/model 的 subagents 将在下次 delegate 时失败，需要手动重新绑定。",
    );
    if (ok) onDelete(provider);
  };

  const handleModelDelete = (model: ModelCatalogEntryDto) => {
    const ok = globalThis.confirm(
      "确定删除此 model？\n注意：已绑定此 model 的 subagents 将在下次 delegate 时失败。",
    );
    if (ok) onModelDelete(model);
  };

  const handleCreateSubmit = () => {
    if (!newModelDraft.modelId.trim()) return;
    const payload: ModelCreateRequestDto = {
      provider_id: provider.id,
      model_id: newModelDraft.modelId.trim(),
      display_name: newModelDraft.modelId.trim(),
      context_window: 200000,
      max_tokens: 8192,
      reasoning: true,
      enabled: true,
      tags: parseTags(newModelDraft.tags),
    };
    onModelCreate(payload);
    setNewModelDraft({ modelId: "", tags: "" });
    setShowCreateModel(false);
  };

  const handleSaveEdit = () => {
    if (!editDraft) return;
    const patch: ProviderUpdateRequestDto = {
      id: provider.id,
      name: editDraft.name,
      base_url: editDraft.base_url,
      provider_kind: editDraft.provider_kind,
    };
    // Three-state api_key: empty string = omit (no change). The "clear key"
    // flow is out of scope for v1. Typing a new value = update.
    if (editDraft.api_key.length > 0) {
      patch.api_key = editDraft.api_key;
    }
    providerMutations.updateProvider.mutate(patch, {
      onSuccess: () => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "provider.updated",
          message: "Provider updated",
          details: { id: provider.id, name: editDraft.name },
        });
        onCancelEdit?.();
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "provider.update.failed",
          message: "Provider update failed",
          details: { id: provider.id, error: err.message },
        });
      },
    });
  };

  // ─── Edit mode render ────────────────────────────────────────────────
  if (isEditing && editDraft) {
    return (
      <div className="provider-card">
        <div className="provider-card__header">
          <input
            type="text"
            value={editDraft.name}
            onChange={(e) => setEditDraft({ ...editDraft, name: e.target.value })}
          />
          <select
            value={editDraft.provider_kind}
            onChange={(e) => setEditDraft({ ...editDraft, provider_kind: e.target.value as ProviderKind })}
          >
            <option value="openai_compatible">openai_compatible</option>
            <option value="anthropic_compatible">anthropic_compatible</option>
          </select>
          <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
            <button type="button" onClick={handleSaveEdit}>保存</button>
            <button type="button" onClick={onCancelEdit}>取消</button>
          </div>
        </div>
        <div style={{ padding: 16, display: "flex", flexDirection: "column", gap: 12 }}>
          <label>
            Base URL
            <input
              type="text"
              value={editDraft.base_url}
              onChange={(e) => setEditDraft({ ...editDraft, base_url: e.target.value })}
            />
          </label>
          <label>
            New API Key (leave empty to keep current)
            <input
              type="password"
              placeholder="new api key (optional)"
              value={editDraft.api_key}
              onChange={(e) => setEditDraft({ ...editDraft, api_key: e.target.value })}
            />
          </label>
          <div style={{ fontFamily: "monospace", fontSize: "0.85rem", color: "var(--color-text-muted)" }}>
            ID: {provider.id}
          </div>
        </div>
        <div className="provider-card__notice">正在编辑 Provider 信息，Models 操作暂不可用</div>
        <div className="provider-card__models provider-card__models--disabled">
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
            <strong>Models</strong>
            <button type="button" disabled>+ Add Model</button>
          </div>
          {models.map((m) => (
            <div key={m.model_db_id} className="model-row">
              <span className="model-row__name">{m.model_id}</span>
              <span>{m.model_enabled ? "●enabled" : "○disabled"}</span>
              <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
                <button type="button" disabled>编辑</button>
                <button type="button" disabled>删除</button>
              </div>
            </div>
          ))}
        </div>
      </div>
    );
  }

  // ─── View mode render ────────────────────────────────────────────────
  return (
    <div className="provider-card">
      <div className="provider-card__header">
        <span className="provider-card__name">{provider.name}</span>
        <span className="chip chip--kind">{KIND_LABEL[provider.provider_kind] ?? provider.provider_kind}</span>
        <span>{provider.enabled ? "● enabled" : "○ disabled"}</span>
        <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
          <button type="button" onClick={onEdit}>编辑</button>
          <button type="button" onClick={() => onTestConnection(provider.id)}>测试连接</button>
          <button type="button" onClick={handleProviderDelete}>删除</button>
        </div>
      </div>
      <div className="provider-card__info">
        <div>{provider.base_url}</div>
        <div style={{ fontFamily: "monospace" }}>ID: {provider.id}</div>
      </div>
      <div className="provider-card__models">
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
          <strong>Models</strong>
          <button type="button" onClick={() => setShowCreateModel((v) => !v)}>+ Add Model</button>
        </div>
        {showCreateModel && (
          <div className="model-row" style={{ flexDirection: "column", alignItems: "stretch", gap: 6 }}>
            <input
              type="text"
              placeholder="model name (e.g. deepseek-chat)"
              value={newModelDraft.modelId}
              onChange={(e) => setNewModelDraft((d) => ({ ...d, modelId: e.target.value }))}
            />
            <input
              type="text"
              placeholder="tags (comma-separated, optional)"
              value={newModelDraft.tags}
              onChange={(e) => setNewModelDraft((d) => ({ ...d, tags: e.target.value }))}
            />
            <div style={{ display: "flex", gap: 8 }}>
              <button type="button" onClick={handleCreateSubmit}>保存</button>
              <button type="button" onClick={() => { setShowCreateModel(false); setNewModelDraft({ modelId: "", tags: "" }); }}>取消</button>
            </div>
          </div>
        )}
        {isModelsLoading ? (
          <div>加载中…</div>
        ) : models.length === 0 && !showCreateModel ? (
          <div style={{ color: "var(--color-text-muted)", fontSize: "0.85rem" }}>暂无 model</div>
        ) : (
          models.map((m) => (
            <div key={m.model_db_id} className="model-row">
              <span className="model-row__name">{m.model_id}</span>
              <span>{m.model_enabled ? "●enabled" : "○disabled"}</span>
              {m.tags.length > 0 && (
                <span className="model-row__tags">
                  {m.tags.map((t) => (
                    <span key={t} className="chip">{t}</span>
                  ))}
                </span>
              )}
              <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
                <button type="button">编辑</button>
                <button type="button" onClick={() => handleModelDelete(m)}>删除</button>
              </div>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `pnpm --filter @busytok/gui test -- --run ProviderCard`
Expected: PASS — all tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/gui/src/components/ProviderCard.tsx apps/gui/src/components/ProviderCard.test.tsx
git commit -m "feat(gui): add inline edit mode to ProviderCard

Header becomes editable (name, base_url, api_key, kind). Models section
stays visible but disabled with a notice. Three-state api_key: empty =
no change, value = update (clear-key flow deferred). Per spec §4."
```

---

## Task 8: Rewrite ProvidersPage to Use New Components + Delete ModelsSection

**Files:**
- Modify: `apps/gui/src/pages/ProvidersPage.tsx` (full rewrite)
- Modify: `apps/gui/src/pages/ProvidersPage.test.tsx` (rewrite tests)
- Delete: `apps/gui/src/components/ModelsSection.tsx`
- Delete: `apps/gui/src/components/ModelsSection.test.tsx`

**Interfaces:**
- Consumes: `ProviderCard`, `ProviderCreationForm`, `useProviders`, `useModels({ includeDisabled: true })`, `useProviderMutations`, `useModelMutations`
- Produces: `ProvidersPage` that renders the new creation form (toggleable) + a list of `ProviderCard`s

- [ ] **Step 1: Write the failing tests for the rewritten ProvidersPage**

Rewrite `apps/gui/src/pages/ProvidersPage.test.tsx`:

```typescript
import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type {
  ModelCatalogEntryDto,
  ModelListResponseDto,
  ProviderDto,
  ProviderListResponseDto,
} from "@busytok/protocol-types";

vi.mock("../api/useBusytokData", () => ({
  useProviders: vi.fn(),
  useProviderMutations: vi.fn(),
  useModels: vi.fn(),
  useModelMutations: vi.fn(),
}));
vi.mock("../logging/safeReporter", () => ({
  reportFrontendEventSafely: vi.fn(),
}));

import { ProvidersPage } from "./ProvidersPage";
import { useProviders, useProviderMutations, useModels, useModelMutations } from "../api/useBusytokData";
import { reportFrontendEventSafely } from "../logging/safeReporter";

const makeProvider = (overrides: Partial<ProviderDto> = {}): ProviderDto => ({
  id: "prov-1",
  name: "deepseek_openai",
  provider_kind: "openai_compatible" as never,
  base_url: "https://api.deepseek.com/v1",
  enabled: true,
  has_api_key: true,
  created_at_ms: 0,
  updated_at_ms: 0,
  ...overrides,
});

const makeModel = (overrides: Partial<ModelCatalogEntryDto> = {}): ModelCatalogEntryDto => ({
  provider_id: "prov-1",
  provider_name: "deepseek_openai",
  provider_kind: "openai_compatible" as never,
  provider_enabled: true,
  model_db_id: "model-db-1",
  model_id: "deepseek-chat",
  model_enabled: true,
  tags: [],
  display_name: "deepseek-chat",
  reasoning: false,
  context_window: 200000,
  max_tokens: 8192,
  ...overrides,
});

const mockMutations = () => ({
  createProvider: { mutate: vi.fn(), isPending: false },
  updateProvider: { mutate: vi.fn(), isPending: false },
  deleteProvider: { mutate: vi.fn((_id: string, opts?: { onSuccess?: () => void }) => { opts?.onSuccess?.(); }), isPending: false },
  testConnection: { mutate: vi.fn((_id: string, opts?: { onSuccess?: (r: unknown) => void; onError?: (e: Error) => void }) => { opts?.onSuccess?.({ ok: true, error: null, models_detected: null }); }), isPending: false },
} as never);

const mockModelMutations = () => ({
  // createModel returns ModelCatalogEntryDto (per useBusytokData.ts:470);
  // the page's onSuccess reads entry.provider_id + entry.model_id, so the
  // mock must pass a concrete entry or the handler throws a TypeError.
  createModel: {
    mutate: vi.fn((_payload: unknown, opts?: { onSuccess?: (entry: ModelCatalogEntryDto) => void }) => {
      opts?.onSuccess?.({
        provider_id: "prov-1",
        provider_name: "deepseek_openai",
        provider_kind: "openai_compatible" as never,
        provider_enabled: true,
        model_db_id: "model-db-new",
        model_id: "new-model",
        model_enabled: true,
        tags: [],
        display_name: "new-model",
        reasoning: false,
        context_window: 200000,
        max_tokens: 8192,
      });
    }),
    isPending: false,
  },
  updateModel: { mutate: vi.fn(), isPending: false },
  deleteModel: { mutate: vi.fn((_id: string, opts?: { onSuccess?: () => void }) => { opts?.onSuccess?.(); }), isPending: false },
  tagsUpdate: { mutate: vi.fn(), isPending: false },
} as never);

function renderPage(overrides: { providers?: ProviderDto[]; models?: ModelCatalogEntryDto[] } = {}) {
  vi.mocked(useProviders).mockReturnValue({
    data: { providers: overrides.providers ?? [] } as ProviderListResponseDto,
    isLoading: false,
    isError: false,
    isFetching: false,
  } as never);
  vi.mocked(useModels).mockReturnValue({
    data: { models: overrides.models ?? [] } as ModelListResponseDto,
    isLoading: false,
    isError: false,
    isFetching: false,
  } as never);
  vi.mocked(useProviderMutations).mockReturnValue(mockMutations());
  vi.mocked(useModelMutations).mockReturnValue(mockModelMutations());

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ProvidersPage />
    </QueryClientProvider>,
  );
}

describe("ProvidersPage (rewritten)", () => {
  it("renders empty state when no providers", () => {
    renderPage();
    expect(screen.getByText(/新建 Provider/i)).toBeInTheDocument();
  });

  it("shows creation form when + 新建 button clicked", () => {
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /\+ 新建/i }));
    expect(screen.getByPlaceholderText(/base url/i)).toBeInTheDocument();
  });

  it("renders a ProviderCard for each provider", () => {
    renderPage({
      providers: [makeProvider({ id: "p1", name: "alpha" }), makeProvider({ id: "p2", name: "beta" })],
    });
    expect(screen.getByText("alpha")).toBeInTheDocument();
    expect(screen.getByText("beta")).toBeInTheDocument();
  });

  it("groups models by provider_id into the correct card", () => {
    renderPage({
      providers: [makeProvider({ id: "p1", name: "alpha" }), makeProvider({ id: "p2", name: "beta" })],
      models: [
        makeModel({ provider_id: "p1", model_id: "alpha-model" }),
        makeModel({ provider_id: "p2", model_id: "beta-model" }),
      ],
    });
    expect(screen.getByText("alpha-model")).toBeInTheDocument();
    expect(screen.getByText("beta-model")).toBeInTheDocument();
  });

  it("emits provider.deleted event on successful delete", () => {
    renderPage({ providers: [makeProvider()] });
    vi.spyOn(globalThis, "confirm").mockReturnValue(true);
    fireEvent.click(screen.getByRole("button", { name: /删除/i }));
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "provider.deleted" }),
    );
  });

  it("emits provider.tested event on successful test connection", () => {
    renderPage({ providers: [makeProvider()] });
    fireEvent.click(screen.getByRole("button", { name: /测试连接/i }));
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "provider.tested" }),
    );
  });

  it("emits provider.test.failed event when testConnection throws (client exception)", () => {
    // Override testConnection to trigger onError (client-side exception).
    vi.mocked(useProviderMutations).mockReturnValue({
      ...mockMutations(),
      testConnection: {
        mutate: vi.fn((_id: string, opts?: { onError?: (e: Error) => void }) => {
          opts?.onError?.(new Error("rpc timeout"));
        }),
        isPending: false,
      },
    } as never);
    renderPage({ providers: [makeProvider()] });
    fireEvent.click(screen.getByRole("button", { name: /测试连接/i }));
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "provider.test.failed", level: "ERROR" }),
    );
  });

  it("emits model.added event on successful model create", () => {
    renderPage({ providers: [makeProvider()] });
    fireEvent.click(screen.getByRole("button", { name: /\+ Add Model/i }));
    fireEvent.change(screen.getByPlaceholderText(/model name/i), { target: { value: "new-model" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "model.added" }),
    );
  });

  it("emits model.deleted event on successful model delete", () => {
    renderPage({
      providers: [makeProvider()],
      models: [makeModel()],
    });
    vi.spyOn(globalThis, "confirm").mockReturnValue(true);
    const deleteButtons = screen.getAllByRole("button", { name: /删除/i });
    // Last delete button is the model row's
    fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "model.deleted" }),
    );
  });

  it("emits model.updated event on successful model update", () => {
    // Override updateModel to trigger onSuccess.
    vi.mocked(useModelMutations).mockReturnValue({
      ...mockModelMutations(),
      updateModel: {
        mutate: vi.fn((_payload: unknown, opts?: { onSuccess?: () => void }) => { opts?.onSuccess?.(); }),
        isPending: false,
      },
    } as never);
    renderPage({ providers: [makeProvider()], models: [makeModel()] });
    // Enter model edit mode, change display_name, save.
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    fireEvent.change(screen.getByDisplayValue("deepseek-chat"), { target: { value: "DeepSeek Chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "model.updated", level: "INFO" }),
    );
  });

  it("emits model.update.failed event on failed model update", () => {
    vi.mocked(useModelMutations).mockReturnValue({
      ...mockModelMutations(),
      updateModel: {
        mutate: vi.fn((_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
          opts?.onError?.(new Error("update failed"));
        }),
        isPending: false,
      },
    } as never);
    renderPage({ providers: [makeProvider()], models: [makeModel()] });
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    fireEvent.change(screen.getByDisplayValue("deepseek-chat"), { target: { value: "DeepSeek Chat" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "model.update.failed", level: "ERROR" }),
    );
  });

  it("emits model.tags.updated event on successful tags update", () => {
    vi.mocked(useModelMutations).mockReturnValue({
      ...mockModelMutations(),
      tagsUpdate: {
        mutate: vi.fn((_payload: unknown, opts?: { onSuccess?: () => void }) => { opts?.onSuccess?.(); }),
        isPending: false,
      },
    } as never);
    renderPage({ providers: [makeProvider()], models: [makeModel()] });
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    fireEvent.change(screen.getByPlaceholderText(/tags/i), { target: { value: "cheap,fast" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "model.tags.updated", level: "INFO" }),
    );
  });

  it("emits model.tags.update.failed event on failed tags update", () => {
    vi.mocked(useModelMutations).mockReturnValue({
      ...mockModelMutations(),
      tagsUpdate: {
        mutate: vi.fn((_payload: unknown, opts?: { onError?: (e: Error) => void }) => {
          opts?.onError?.(new Error("tags update failed"));
        }),
        isPending: false,
      },
    } as never);
    renderPage({ providers: [makeProvider()], models: [makeModel()] });
    const editButtons = screen.getAllByRole("button", { name: /编辑/i });
    fireEvent.click(editButtons[editButtons.length - 1]);
    fireEvent.change(screen.getByPlaceholderText(/tags/i), { target: { value: "cheap,fast" } });
    fireEvent.click(screen.getByRole("button", { name: /^保存$/i }));
    expect(vi.mocked(reportFrontendEventSafely)).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "model.tags.update.failed", level: "ERROR" }),
    );
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --filter @busytok/gui test -- --run ProvidersPage`
Expected: FAIL — old ProvidersPage doesn't match new structure.

- [ ] **Step 3: Rewrite ProvidersPage**

Replace `apps/gui/src/pages/ProvidersPage.tsx` with:

```typescript
import { useMemo, useState } from "react";
import type {
  ModelCatalogEntryDto,
  ModelCreateRequestDto,
  ModelUpdateRequestDto,
  ProviderDto,
  ProviderUpdateRequestDto,
} from "@busytok/protocol-types";
import {
  useModelMutations,
  useModels,
  useProviderMutations,
  useProviders,
} from "../api/useBusytokData";
import { ProviderCard } from "../components/ProviderCard";
import { ProviderCreationForm } from "../components/ProviderCreationForm";
import { reportFrontendEventSafely } from "../logging/safeReporter";

export function ProvidersPage() {
  const providersQuery = useProviders();
  const modelsQuery = useModels({ includeDisabled: true });
  const providerMutations = useProviderMutations();
  const modelMutations = useModelMutations();

  const [showCreateForm, setShowCreateForm] = useState(false);
  const [editingProviderId, setEditingProviderId] = useState<string | null>(null);

  // Group models by provider_id once per render
  const modelsByProvider = useMemo(() => {
    const map = new Map<string, ModelCatalogEntryDto[]>();
    for (const m of modelsQuery.data?.models ?? []) {
      const list = map.get(m.provider_id) ?? [];
      list.push(m);
      map.set(m.provider_id, list);
    }
    return map;
  }, [modelsQuery.data]);

  const handleProviderDelete = (provider: ProviderDto) => {
    providerMutations.deleteProvider.mutate(provider.id, {
      onSuccess: () => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "provider.deleted",
          message: "Provider deleted",
          details: { id: provider.id, name: provider.name },
        });
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "provider.delete.failed",
          message: "Provider delete failed",
          details: { id: provider.id, name: provider.name, error: err.message },
        });
      },
    });
  };

  const handleTestConnection = (id: string) => {
    providerMutations.testConnection.mutate(id, {
      onSuccess: (response) => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "provider.tested",
          message: "Provider connection test completed",
          details: { id, ok: response.ok, error: response.error },
        });
      },
      onError: (err: Error) => {
        // Client-side exception (RPC call itself failed) → provider.test.failed.
        // RPC-returned ok:false is NOT an error here — it goes through onSuccess
        // and emits provider.tested with ok:false (preserves current semantics).
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "provider.test.failed",
          message: "Provider connection test failed (client exception)",
          details: { id, error: err.message },
        });
      },
    });
  };

  const handleModelCreate = (payload: ModelCreateRequestDto) => {
    modelMutations.createModel.mutate(payload, {
      onSuccess: (entry) => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "model.added",
          message: "Model added",
          details: { provider_id: entry.provider_id, model_id: entry.model_id },
        });
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "model.add.failed",
          message: "Model creation failed",
          details: { provider_id: payload.provider_id, model_id: payload.model_id, error: err.message },
        });
      },
    });
  };

  const handleModelUpdate = (model: ModelCatalogEntryDto, patch: ModelUpdateRequestDto) => {
    modelMutations.updateModel.mutate({ ...patch, id: model.model_db_id }, {
      onSuccess: () => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "model.updated",
          message: "Model updated",
          details: { provider_id: model.provider_id, model_id: model.model_id },
        });
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "model.update.failed",
          message: "Model update failed",
          details: { provider_id: model.provider_id, model_id: model.model_id, error: err.message },
        });
      },
    });
  };

  const handleModelTagsUpdate = (modelId: string, tags: string[]) => {
    modelMutations.tagsUpdate.mutate({ modelId, tags }, {
      onSuccess: () => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "model.tags.updated",
          message: "Model tags updated",
          details: { model_id: modelId, tags },
        });
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "model.tags.update.failed",
          message: "Model tags update failed",
          details: { model_id: modelId, error: err.message },
        });
      },
    });
  };

  const handleModelDelete = (model: ModelCatalogEntryDto) => {
    modelMutations.deleteModel.mutate(model.model_db_id, {
      onSuccess: () => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "model.deleted",
          message: "Model deleted",
          details: { provider_id: model.provider_id, model_id: model.model_id },
        });
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "model.delete.failed",
          message: "Model delete failed",
          details: { provider_id: model.provider_id, model_id: model.model_id, error: err.message },
        });
      },
    });
  };

  return (
    <div className="settings-page">
      <div className="settings-pane">
        <div className="settings-section">
          <h2>Providers</h2>
          <button type="button" onClick={() => setShowCreateForm((v) => !v)}>+ 新建</button>
        </div>

        {showCreateForm && (
          <ProviderCreationForm onClose={() => setShowCreateForm(false)} />
        )}

        {(providersQuery.data?.providers ?? []).map((provider) => (
          <ProviderCard
            key={provider.id}
            provider={provider}
            models={modelsByProvider.get(provider.id) ?? []}
            isModelsLoading={modelsQuery.isLoading}
            providerMutations={providerMutations}
            modelMutations={modelMutations}
            onEdit={() => setEditingProviderId(provider.id)}
            onTestConnection={handleTestConnection}
            onDelete={handleProviderDelete}
            onModelCreate={handleModelCreate}
            onModelUpdate={handleModelUpdate}
            onModelTagsUpdate={handleModelTagsUpdate}
            onModelDelete={handleModelDelete}
            isEditing={editingProviderId === provider.id}
            onCancelEdit={() => setEditingProviderId(null)}
          />
        ))}
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Delete ModelsSection (no longer imported)**

```bash
rm apps/gui/src/components/ModelsSection.tsx apps/gui/src/components/ModelsSection.test.tsx
```

- [ ] **Step 5: Run all GUI tests**

Run: `pnpm --filter @busytok/gui test`
Expected: PASS.

- [ ] **Step 7: Run typecheck, build, and coverage gate**

Run: `pnpm --filter @busytok/gui typecheck && pnpm --filter @busytok/gui build && pnpm coverage:gui`
Expected: PASS — typecheck clean, build succeeds, coverage ≥90% lines.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(gui): rewrite ProvidersPage with always-expanded provider cards

Replaces the old ProviderRow + separate ModelsSection with a single
list of ProviderCard components. Models are fetched once via
useModels({ includeDisabled: true }) and grouped by provider_id. Edit
mode is tracked at the page level. Per spec §4. Deletes ModelsSection
(functionality absorbed into ProviderCard). All provider/model CRUD
mutations now emit reportFrontendEventSafely events per spec §8."
```

---

## Task 9: CLI ProviderCommand Enum + Dispatch Wiring

**Files:**
- Modify: `apps/cli/src/main.rs` (add enums + variant + dispatch arm + `command_name` arm)
- Modify: `apps/cli/src/commands/mod.rs` (add `pub mod provider;`)
- Create: `apps/cli/src/commands/provider.rs` (stub with `handle` dispatcher only — handlers filled in Task 10)

**Interfaces:**
- Produces:
  - `ProviderCommand` enum (List / Add / Show / Update / Delete / Test / Model)
  - `ProviderModelCommand` enum (List / Add / Update / Delete)
  - `commands::provider::handle(cmd: ProviderCommand) -> Result<()>` dispatcher

- [ ] **Step 1: Write the failing parser tests**

In `apps/cli/src/main.rs`, append to the test module:

```rust
#[test]
fn args_parses_provider_list() {
    let args = Args::try_parse_from(["busytok", "provider", "list"]).unwrap();
    match args.command {
        Some(Command::Provider { subcommand }) => {
            assert!(matches!(subcommand, ProviderCommand::List { json: false }));
        }
        other => panic!("expected Provider, got: {other:?}"),
    }
}

#[test]
fn args_parses_provider_add_with_required_flags() {
    let args = Args::try_parse_from([
        "busytok", "provider", "add",
        "--url", "https://api.deepseek.com/v1",
        "--key", "sk-test",
    ]).unwrap();
    match args.command {
        Some(Command::Provider { subcommand: ProviderCommand::Add { url, key, kind, name, model, tags } }) => {
            assert_eq!(url, "https://api.deepseek.com/v1");
            assert_eq!(key, "sk-test");
            assert_eq!(kind, "openai_compatible");
            assert!(name.is_none());
            assert!(model.is_none());
            assert!(tags.is_none());
        }
        other => panic!("expected Provider Add, got: {other:?}"),
    }
}

#[test]
fn args_parses_provider_add_with_all_flags() {
    let args = Args::try_parse_from([
        "busytok", "provider", "add",
        "--url", "https://api.deepseek.com/v1",
        "--key", "sk-test",
        "--kind", "anthropic_compatible",
        "--name", "custom_name",
        "--model", "claude-3-opus",
        "--tags", "fast,reasoning",
    ]).unwrap();
    match args.command {
        Some(Command::Provider { subcommand: ProviderCommand::Add { url, key, kind, name, model, tags } }) => {
            assert_eq!(kind, "anthropic_compatible");
            assert_eq!(name.as_deref(), Some("custom_name"));
            assert_eq!(model.as_deref(), Some("claude-3-opus"));
            assert_eq!(tags.as_deref(), Some("fast,reasoning"));
            (url, key); // unused
        }
        other => panic!("expected Provider Add, got: {other:?}"),
    }
}

#[test]
fn args_parses_provider_delete_with_yes_flag() {
    let args = Args::try_parse_from(["busytok", "provider", "delete", "prov-1", "--yes"]).unwrap();
    match args.command {
        Some(Command::Provider { subcommand: ProviderCommand::Delete { id, yes } }) => {
            assert_eq!(id, "prov-1");
            assert!(yes);
        }
        other => panic!("expected Provider Delete, got: {other:?}"),
    }
}

#[test]
fn args_parses_provider_delete_without_yes_flag() {
    let args = Args::try_parse_from(["busytok", "provider", "delete", "prov-1"]).unwrap();
    match args.command {
        Some(Command::Provider { subcommand: ProviderCommand::Delete { yes, .. } }) => {
            assert!(!yes);
        }
        other => panic!("expected Provider Delete, got: {other:?}"),
    }
}

#[test]
fn args_parses_provider_model_add() {
    let args = Args::try_parse_from([
        "busytok", "provider", "model", "add", "prov-1",
        "--name", "deepseek-chat",
    ]).unwrap();
    match args.command {
        Some(Command::Provider { subcommand: ProviderCommand::Model { subcommand: ProviderModelCommand::Add { provider_id, name, tags, context_window, max_tokens, reasoning, display_name } } }) => {
            assert_eq!(provider_id, "prov-1");
            assert_eq!(name, "deepseek-chat");
            assert!(tags.is_none());
            assert!(context_window.is_none());
            assert!(max_tokens.is_none());
            assert!(reasoning);
            assert!(display_name.is_none());
        }
        other => panic!("expected Provider Model Add, got: {other:?}"),
    }
}

#[test]
fn args_parses_provider_model_delete_with_yes() {
    let args = Args::try_parse_from([
        "busytok", "provider", "model", "delete", "prov-1", "deepseek-chat", "--yes",
    ]).unwrap();
    match args.command {
        Some(Command::Provider { subcommand: ProviderCommand::Model { subcommand: ProviderModelCommand::Delete { provider_id, model_id, yes } } }) => {
            assert_eq!(provider_id, "prov-1");
            assert_eq!(model_id, "deepseek-chat");
            assert!(yes);
        }
        other => panic!("expected Provider Model Delete, got: {other:?}"),
    }
}

#[test]
fn command_name_returns_provider_for_provider_variant() {
    let args = Args::try_parse_from(["busytok", "provider", "list"]).unwrap();
    assert_eq!(command_name(args.command.as_ref().unwrap()), "provider");
}

#[test]
fn args_rejects_provider_add_with_invalid_kind() {
    let result = Args::try_parse_from([
        "busytok", "provider", "add",
        "--url", "https://api.deepseek.com",
        "--key", "sk-test",
        "--kind", "invalid_kind",
    ]);
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p busytok-cli --bin busytok -- args_parses_provider`
Expected: FAIL — `ProviderCommand` and `Command::Provider` don't exist yet.

- [ ] **Step 3: Add the enums and dispatch to main.rs**

In `apps/cli/src/main.rs`:

Add after the existing `Command` enum (after line 135, before the closing brace):

```rust
    /// Manage providers and their models
    Provider {
        #[command(subcommand)]
        subcommand: ProviderCommand,
    },
```

Add the new enums after the `Command` enum (in the same file, after the closing brace of `Command`):

```rust
#[derive(Debug, Subcommand)]
enum ProviderCommand {
    /// List all providers
    List {
        #[arg(long)]
        json: bool,
    },
    /// Create a new provider
    Add {
        #[arg(long)]
        url: String,
        #[arg(long)]
        key: String,
        #[arg(long, default_value = "openai_compatible", value_parser = ["openai_compatible", "anthropic_compatible"])]
        kind: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        tags: Option<String>,
    },
    /// Show provider details
    Show {
        id: String,
    },
    /// Update a provider
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        key: Option<String>,
        #[arg(long, value_parser = ["openai_compatible", "anthropic_compatible"])]
        kind: Option<String>,
        #[arg(long)]
        enabled: Option<bool>,
    },
    /// Delete a provider (cascades to models; may break bound subagents)
    Delete {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Test connection to a provider
    Test {
        id: String,
    },
    /// Manage models under a provider
    Model {
        #[command(subcommand)]
        subcommand: ProviderModelCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ProviderModelCommand {
    /// List models for a provider (includes disabled models)
    List {
        provider_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Add a model to a provider
    Add {
        provider_id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long)]
        context_window: Option<i64>,
        #[arg(long)]
        max_tokens: Option<i64>,
        #[arg(long, default_value = "true")]
        reasoning: bool,
        #[arg(long)]
        display_name: Option<String>,
    },
    /// Update a model
    Update {
        provider_id: String,
        model_id: String,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long)]
        context_window: Option<i64>,
        #[arg(long)]
        max_tokens: Option<i64>,
        #[arg(long)]
        reasoning: Option<bool>,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long)]
        display_name: Option<String>,
    },
    /// Delete a model (may break bound subagents)
    Delete {
        provider_id: String,
        model_id: String,
        #[arg(long)]
        yes: bool,
    },
}
```

Add the dispatch arm in the `run()` match block (next to the `Command::Models` arm):

```rust
        Command::Provider { subcommand } => commands::provider::handle(subcommand).await,
```

Add the `command_name` arm:

```rust
        Command::Provider { .. } => "provider",
```

- [ ] **Step 4: Add `pub mod provider;` to commands/mod.rs**

In `apps/cli/src/commands/mod.rs`, change line 20 from `pub mod models;` to:

```rust
pub mod models;
pub mod provider;
```

- [ ] **Step 5: Create the stub handler module**

Create `apps/cli/src/commands/provider.rs`:

```rust
//! Handler for `busytok provider` — manage providers and their models.
use anyhow::Result;
use crate::ProviderCommand;

/// Dispatch a `ProviderCommand` to its handler.
pub async fn handle(cmd: ProviderCommand) -> Result<()> {
    match cmd {
        ProviderCommand::List { json } => handle_list(json).await,
        ProviderCommand::Add { url, key, kind, name, model, tags } => {
            handle_add(url, key, kind, name, model, tags).await
        }
        ProviderCommand::Show { id } => handle_show(id).await,
        ProviderCommand::Update { id, name, url, key, kind, enabled } => {
            handle_update(id, name, url, key, kind, enabled).await
        }
        ProviderCommand::Delete { id, yes } => handle_delete(id, yes).await,
        ProviderCommand::Test { id } => handle_test(id).await,
        ProviderCommand::Model { subcommand } => handle_model(subcommand).await,
    }
}

async fn handle_list(_json: bool) -> Result<()> {
    anyhow::bail!("not yet implemented")
}
async fn handle_add(_url: String, _key: String, _kind: String, _name: Option<String>, _model: Option<String>, _tags: Option<String>) -> Result<()> {
    anyhow::bail!("not yet implemented")
}
async fn handle_show(_id: String) -> Result<()> {
    anyhow::bail!("not yet implemented")
}
async fn handle_update(_id: String, _name: Option<String>, _url: Option<String>, _key: Option<String>, _kind: Option<String>, _enabled: Option<bool>) -> Result<()> {
    anyhow::bail!("not yet implemented")
}
async fn handle_delete(_id: String, _yes: bool) -> Result<()> {
    anyhow::bail!("not yet implemented")
}
async fn handle_test(_id: String) -> Result<()> {
    anyhow::bail!("not yet implemented")
}

async fn handle_model(_subcommand: crate::ProviderModelCommand) -> Result<()> {
    anyhow::bail!("not yet implemented")
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p busytok-cli --bin busytok -- args_parses_provider command_name_returns_provider args_rejects_provider_add_with_invalid_kind`
Expected: PASS — all parser tests pass.

- [ ] **Step 7: Run clippy and fmt**

Run: `cargo fmt --all && cargo clippy -p busytok-cli -- -D warnings`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add apps/cli/src/main.rs apps/cli/src/commands/mod.rs apps/cli/src/commands/provider.rs
git commit -m "feat(cli): add provider subcommand enum + dispatch wiring

Introduces ProviderCommand and ProviderModelCommand enums, Command::Provider
variant, dispatch arm, and a stub commands::provider module. Parser tests
cover all variants and the --yes flag for delete commands. Per spec §7."
```

---

## Task 10: CLI Provider Handlers (list/add/show/update/delete/test)

**Files:**
- Modify: `apps/cli/src/commands/provider.rs`

**Interfaces:**
- Consumes:
  - `connect_client` from `super::` (the `pub(crate)` helper in `commands/mod.rs`)
  - `ControlRequest`, `ControlResponse` from `busytok_protocol`
  - DTOs: `ProviderDto`, `ProviderListResponseDto`, `ProviderCreateRequestDto`, `ProviderUpdateRequestDto`, `ProviderTestConnectionResponseDto`
  - `ProviderKind` from `busytok_domain` (NOT re-exported from `busytok_protocol::dto`)
  - `std::io::IsTerminal` (stable since Rust 1.70; workspace is 1.88)
- Produces: working handlers for List / Add / Show / Update / Delete / Test

- [ ] **Step 1: Write the failing handler tests**

Append to `apps/cli/src/commands/provider.rs` (test module at the bottom):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use busytok_control::dispatch::RuntimeControl;
    use busytok_control::server::ControlServer;
    use busytok_control::TestRuntimeControl;
    use busytok_domain::ProviderKind;
    use busytok_protocol::dto::*;
    use serial_test::serial;
    use std::sync::Arc;

    // ─── Name derivation unit tests ──────────────────────────────────

    #[test]
    fn derive_provider_name_from_typical_url() {
        assert_eq!(
            derive_provider_name("https://api.deepseek.com/v1", "openai_compatible"),
            Some("deepseek_openai".to_string())
        );
    }

    #[test]
    fn derive_provider_name_strips_compatible_suffix() {
        assert_eq!(
            derive_provider_name("https://api.anthropic.com", "anthropic_compatible"),
            Some("anthropic_anthropic".to_string())
        );
    }

    #[test]
    fn derive_provider_name_falls_back_for_single_part_host() {
        assert_eq!(
            derive_provider_name("https://localhost:8080/v1", "openai_compatible"),
            Some("localhost_openai".to_string())
        );
    }

    #[test]
    fn derive_provider_name_handles_port() {
        assert_eq!(
            derive_provider_name("http://my.api.host:3000", "openai_compatible"),
            Some("host_openai".to_string())
        );
    }

    #[test]
    fn derive_unique_provider_name_no_collision() {
        let existing: std::collections::HashSet<String> = ["other_openai".to_string()].into_iter().collect();
        assert_eq!(
            derive_unique_provider_name("https://api.deepseek.com", "openai_compatible", &existing),
            "deepseek_openai"
        );
    }

    #[test]
    fn derive_unique_provider_name_appends_2_on_collision() {
        let existing: std::collections::HashSet<String> = ["deepseek_openai".to_string()].into_iter().collect();
        assert_eq!(
            derive_unique_provider_name("https://api.deepseek.com", "openai_compatible", &existing),
            "deepseek_openai_2"
        );
    }

    #[test]
    fn derive_unique_provider_name_increments_until_unique() {
        let existing: std::collections::HashSet<String> = [
            "deepseek_openai".to_string(),
            "deepseek_openai_2".to_string(),
            "deepseek_openai_3".to_string(),
        ].into_iter().collect();
        assert_eq!(
            derive_unique_provider_name("https://api.deepseek.com", "openai_compatible", &existing),
            "deepseek_openai_4"
        );
    }

    // ─── URL validation unit tests ───────────────────────────────────

    #[test]
    fn validate_base_url_accepts_https() {
        assert!(validate_base_url("https://api.deepseek.com/v1").is_ok());
    }

    #[test]
    fn validate_base_url_accepts_http() {
        assert!(validate_base_url("http://localhost:8080").is_ok());
    }

    #[test]
    fn validate_base_url_rejects_empty() {
        assert!(validate_base_url("").is_err());
    }

    #[test]
    fn validate_base_url_rejects_missing_protocol() {
        assert!(validate_base_url("api.deepseek.com").is_err());
    }

    #[test]
    fn validate_base_url_rejects_ftp() {
        assert!(validate_base_url("ftp://api.deepseek.com").is_err());
    }

    // ─── Tag parsing ─────────────────────────────────────────────────

    #[test]
    fn parse_tags_empty_returns_empty_vec() {
        assert!(parse_tags("").is_empty());
    }

    #[test]
    fn parse_tags_splits_and_trims() {
        assert_eq!(parse_tags("cheap, fast , reasoning"), vec!["cheap", "fast", "reasoning"]);
    }

    #[test]
    fn parse_tags_drops_empty_entries() {
        assert_eq!(parse_tags("cheap,,fast,"), vec!["cheap", "fast"]);
    }

    // ─── Handler integration tests (against in-process ControlServer) ─

    struct ProvidersRuntime {
        inner: TestRuntimeControl,
        providers: Vec<ProviderDto>,
    }

    #[async_trait]
    impl RuntimeControl for ProvidersRuntime {
        async fn provider_list(&self) -> anyhow::Result<ProviderListResponseDto> {
            Ok(ProviderListResponseDto { providers: self.providers.clone() })
        }
        // Everything else delegates to the inner runtime. The boilerplate is
        // verbatim from commands/models.rs:288-533 (only the struct name and
        // the overridden method change).
        async fn service_health(&self) -> anyhow::Result<ServiceHealthDto> {
            self.inner.service_health().await
        }
        async fn service_status(&self) -> anyhow::Result<ServiceStatusDto> {
            self.inner.service_status().await
        }
        async fn shell_status(&self) -> anyhow::Result<ShellStatusDto> {
            self.inner.shell_status().await
        }
        async fn overview_summary(
            &self,
            req: OverviewSummaryRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<OverviewSummaryDto>> {
            self.inner.overview_summary(req).await
        }
        async fn overview_trend(
            &self,
            req: OverviewTrendRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<OverviewTrendResponseDto>> {
            self.inner.overview_trend(req).await
        }
        async fn overview_heatmap(
            &self,
            req: OverviewHeatmapRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<OverviewHeatmapResponseDto>> {
            self.inner.overview_heatmap(req).await
        }
        async fn overview_rankings(
            &self,
            req: OverviewRankingsRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<OverviewRankingsResponseDto>> {
            self.inner.overview_rankings(req).await
        }
        async fn receipt_daily(
            &self,
            req: ReceiptDailyRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<ReceiptDailyDto>> {
            self.inner.receipt_daily(req).await
        }
        async fn activity_recent(
            &self,
            req: ActivityRecentRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<ActivityRecentResponseDto>> {
            self.inner.activity_recent(req).await
        }
        async fn activity_list(
            &self,
            req: ActivityListRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<ActivityListResponseDto>> {
            self.inner.activity_list(req).await
        }
        async fn activity_detail(
            &self,
            req: ActivityDetailRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<ActivityDetailDto>> {
            self.inner.activity_detail(req).await
        }
        async fn breakdown_list(
            &self,
            req: BreakdownListRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<BreakdownListResponseDto>> {
            self.inner.breakdown_list(req).await
        }
        async fn breakdown_detail(
            &self,
            req: BreakdownDetailRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<BreakdownDetailDto>> {
            self.inner.breakdown_detail(req).await
        }
        async fn clients_snapshot(
            &self,
            req: ClientsSnapshotRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<ClientsSnapshotDto>> {
            self.inner.clients_snapshot(req).await
        }
        async fn clients_detail(
            &self,
            req: ClientSourceDetailRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<ClientSourceDetailDto>> {
            self.inner.clients_detail(req).await
        }
        async fn settings_snapshot(&self) -> anyhow::Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
            self.inner.settings_snapshot().await
        }
        async fn settings_update(
            &self,
            req: SettingsUpdateRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
            self.inner.settings_update(req).await
        }
        async fn settings_diagnostics(&self) -> anyhow::Result<ReadEnvelopeDto<SettingsDiagnosticsDto>> {
            self.inner.settings_diagnostics().await
        }
        async fn settings_recovery_action(
            &self,
            req: SettingsRecoveryActionRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<SettingsRecoveryActionResponseDto>> {
            self.inner.settings_recovery_action(req).await
        }
        async fn live_window(
            &self,
            req: LiveWindowRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<LiveWindowDto>> {
            self.inner.live_window(req).await
        }
        async fn prompts_list(
            &self,
            req: PromptListQueryDto,
        ) -> anyhow::Result<ReadEnvelopeDto<PromptListResponseDto>> {
            self.inner.prompts_list(req).await
        }
        async fn prompts_get(
            &self,
            req: PromptGetRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
            self.inner.prompts_get(req).await
        }
        async fn prompts_create(
            &self,
            req: PromptCreateRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
            self.inner.prompts_create(req).await
        }
        async fn prompts_update(
            &self,
            req: PromptUpdateRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
            self.inner.prompts_update(req).await
        }
        async fn prompts_delete(
            &self,
            req: PromptDeleteRequestDto,
        ) -> anyhow::Result<PromptDeleteResultDto> {
            self.inner.prompts_delete(req).await
        }
        async fn prompts_use(&self, req: PromptUseRequestDto) -> anyhow::Result<PromptUseResultDto> {
            self.inner.prompts_use(req).await
        }
        async fn suggest_tags(
            &self,
            req: PromptSuggestTagsRequestDto,
        ) -> anyhow::Result<PromptSuggestTagsResponseDto> {
            self.inner.suggest_tags(req).await
        }
        async fn subagent_delegate(
            &self,
            req: SubagentDelegateRequestDto,
        ) -> anyhow::Result<SubagentDelegateResponseDto> {
            self.inner.subagent_delegate(req).await
        }
        async fn subagent_list(
            &self,
            req: SubagentListRequestDto,
        ) -> anyhow::Result<SubagentListResponseDto> {
            self.inner.subagent_list(req).await
        }
        async fn subagent_show(&self, req: SubagentResolveRequestDto) -> anyhow::Result<SubagentDetailDto> {
            self.inner.subagent_show(req).await
        }
        async fn subagent_tasks(
            &self,
            req: SubagentTasksRequestDto,
        ) -> anyhow::Result<SubagentTasksResponseDto> {
            self.inner.subagent_tasks(req).await
        }
        async fn subagent_hibernate(
            &self,
            req: SubagentResolveRequestDto,
        ) -> anyhow::Result<SubagentAckDto> {
            self.inner.subagent_hibernate(req).await
        }
        async fn subagent_delete(&self, req: SubagentDeleteRequestDto) -> anyhow::Result<SubagentAckDto> {
            self.inner.subagent_delete(req).await
        }
        async fn subagent_runtime_status(
            &self,
            req: SubagentRuntimeStatusRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<SubagentRuntimeStatusDto>> {
            self.inner.subagent_runtime_status(req).await
        }
        async fn subagent_task_get(
            &self,
            req: SubagentTaskGetRequestDto,
        ) -> anyhow::Result<SubagentTaskDetailDto> {
            self.inner.subagent_task_get(req).await
        }
        async fn provider_create(&self, req: ProviderCreateRequestDto) -> anyhow::Result<ProviderDto> {
            self.inner.provider_create(req).await
        }
        async fn provider_update(&self, req: ProviderUpdateRequestDto) -> anyhow::Result<ProviderDto> {
            self.inner.provider_update(req).await
        }
        async fn provider_delete(&self, req: ProviderDeleteRequestDto) -> anyhow::Result<()> {
            self.inner.provider_delete(req).await
        }
        async fn provider_test_connection(
            &self,
            req: ProviderTestConnectionRequestDto,
        ) -> anyhow::Result<ProviderTestConnectionResponseDto> {
            self.inner.provider_test_connection(req).await
        }
        async fn model_create(&self, req: ModelCreateRequestDto) -> anyhow::Result<ModelCatalogEntryDto> {
            self.inner.model_create(req).await
        }
        async fn model_list(&self, req: ModelListRequestDto) -> anyhow::Result<ModelListResponseDto> {
            self.inner.model_list(req).await
        }
        async fn model_update(&self, req: ModelUpdateRequestDto) -> anyhow::Result<()> {
            self.inner.model_update(req).await
        }
        async fn model_delete(&self, req: ModelDeleteRequestDto) -> anyhow::Result<()> {
            self.inner.model_delete(req).await
        }
        async fn model_tags_update(&self, req: ModelTagUpdateDto) -> anyhow::Result<()> {
            self.inner.model_tags_update(req).await
        }
        async fn pi_sidecar_locator_update(
            &self,
            req: PiSidecarLocatorUpdateRequestDto,
        ) -> anyhow::Result<PiSidecarLocatorUpdateResponseDto> {
            self.inner.pi_sidecar_locator_update(req).await
        }
        async fn profile_create(&self, req: ProfileCreateRequestDto) -> anyhow::Result<ProfileDto> {
            self.inner.profile_create(req).await
        }
        async fn profile_update(&self, req: ProfileUpdateRequestDto) -> anyhow::Result<ProfileDto> {
            self.inner.profile_update(req).await
        }
        async fn profile_delete(&self, req: ProfileDeleteRequestDto) -> anyhow::Result<()> {
            self.inner.profile_delete(req).await
        }
    }

    // Helper: spawn a test server with canned providers
    async fn spawn_providers_server(providers: Vec<ProviderDto>) -> (ControlServer, String) {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(ProvidersRuntime {
            inner,
            providers,
        });
        ControlServer::spawn_for_test(runtime).await.unwrap()
    }

    fn sample_provider() -> ProviderDto {
        ProviderDto {
            id: "prov-1".to_string(),
            name: "deepseek_openai".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.deepseek.com/v1".to_string(),
            enabled: true,
            has_api_key: true,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[tokio::test]
    #[serial]
    async fn handle_list_succeeds_with_providers() {
        let (harness, socket) = spawn_providers_server(vec![sample_provider()]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_list(false).await;
        drop(harness);
        assert!(result.is_ok(), "handle_list failed: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_list_json_succeeds() {
        let (harness, socket) = spawn_providers_server(vec![sample_provider()]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_list(true).await;
        drop(harness);
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[serial]
    async fn handle_list_empty_providers_succeeds() {
        let (harness, socket) = spawn_providers_server(vec![]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_list(false).await;
        drop(harness);
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[serial]
    async fn handle_delete_proceeds_with_yes_flag() {
        let (harness, socket) = spawn_providers_server(vec![sample_provider()]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_delete("prov-1".to_string(), true).await;
        drop(harness);
        assert!(result.is_ok(), "delete with --yes should proceed: {:?}", result);
    }

    // ── Pure confirmation logic tests (no TTY/stdin/IO needed) ──────────

    #[test]
    fn confirmation_proceeds_with_yes_flag() {
        // --yes always proceeds, regardless of TTY or input.
        assert!(matches!(
            evaluate_delete_confirmation(true, false, ""),
            DeleteConfirmation::Proceed
        ));
        assert!(matches!(
            evaluate_delete_confirmation(true, true, "no\n"),
            DeleteConfirmation::Proceed
        ));
    }

    #[test]
    fn confirmation_bails_in_non_tty_without_yes() {
        // Non-interactive mode without --yes must bail — this is the safety
        // guarantee that prevents accidental deletes in CI/scripts.
        assert!(matches!(
            evaluate_delete_confirmation(false, false, ""),
            DeleteConfirmation::Bail
        ));
    }

    #[test]
    fn confirmation_proceeds_in_tty_with_yes_input() {
        // TTY + user types "yes" → proceed.
        assert!(matches!(
            evaluate_delete_confirmation(false, true, "yes\n"),
            DeleteConfirmation::Proceed
        ));
    }

    #[test]
    fn confirmation_cancels_in_tty_with_non_yes_input() {
        // TTY + user types anything other than "yes" → cancel (no error).
        assert!(matches!(
            evaluate_delete_confirmation(false, true, "no\n"),
            DeleteConfirmation::Cancel
        ));
        assert!(matches!(
            evaluate_delete_confirmation(false, true, "\n"),
            DeleteConfirmation::Cancel
        ));
    }

    #[tokio::test]
    #[serial]
    async fn handle_show_succeeds_for_existing_provider() {
        let (harness, socket) = spawn_providers_server(vec![sample_provider()]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_show("prov-1".to_string()).await;
        drop(harness);
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[serial]
    async fn handle_show_fails_for_missing_provider() {
        let (harness, socket) = spawn_providers_server(vec![]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_show("nonexistent".to_string()).await;
        drop(harness);
        assert!(result.is_err());
    }
}
```

**Note on the `ProvidersRuntime` wrapper:** The full trait delegation (all ~35 `RuntimeControl` methods) is included verbatim above — copied from the `commands/models.rs:288-533` pattern, with only `provider_list` overridden to return the canned `providers` field. When a later test needs to override a different method (e.g. `provider_create` to simulate a create failure), copy the wrapper, rename the struct (e.g. `FailingCreateRuntime`), and override only that method — keep all other delegations identical.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p busytok-cli --bin busytok -- commands::provider`
Expected: FAIL — handlers are stubs.

- [ ] **Step 3: Implement the provider handlers**

Replace `apps/cli/src/commands/provider.rs` with the full implementation. Add these imports at the top:

```rust
//! Handler for `busytok provider` — manage providers and their models.
use std::io::IsTerminal;

use anyhow::{Context, Result};
use busytok_domain::ProviderKind;
use busytok_protocol::dto::{
    ModelCreateRequestDto, ModelListRequestDto, ModelListResponseDto, ModelUpdateRequestDto,
    ProviderCreateRequestDto, ProviderDto, ProviderListResponseDto,
    ProviderTestConnectionResponseDto, ProviderUpdateRequestDto,
};
use busytok_protocol::{ControlRequest, ControlResponse};

use super::connect_client;
use crate::{ProviderCommand, ProviderModelCommand};
```

Then implement each handler. The pure helpers (`parse_tags`, `extract_host`, `derive_provider_name`, `derive_unique_provider_name`, `validate_base_url`) go in the same file (no separate module needed — the CLI is a single binary and these are private):

```rust
fn parse_tags(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn validate_base_url(input: &str) -> Result<()> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Base URL cannot be empty");
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        anyhow::bail!("URL must start with http:// or https://");
    }
    Ok(())
}

fn extract_host(url: &str) -> Option<&str> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let before_path = after_scheme.split('/').next()?;
    let before_colon = before_path.split(':').next()?;
    Some(before_colon)
}

fn derive_provider_name(url: &str, kind: &str) -> Option<String> {
    let host = extract_host(url)?;
    let parts: Vec<&str> = host.split('.').collect();
    let domain = parts.get(parts.len().saturating_sub(2)).copied().unwrap_or(host);
    let kind_short = kind.replace("_compatible", "");
    Some(format!("{}_{}", domain, kind_short))
}

fn derive_unique_provider_name(
    url: &str,
    kind: &str,
    existing_names: &std::collections::HashSet<String>,
) -> String {
    let base = derive_provider_name(url, kind).unwrap_or_else(|| "provider".to_string());
    if !existing_names.contains(&base) {
        return base;
    }
    let mut i = 2;
    while existing_names.contains(&format!("{}_{}", base, i)) {
        i += 1;
    }
    format!("{}_{}", base, i)
}

/// Decision returned by `evaluate_delete_confirmation` — a pure enum so the
/// safety semantics are unit-testable without TTY/stdin/IO.
enum DeleteConfirmation {
    Proceed,
    Cancel,
    Bail,
}

/// Pure confirmation logic for destructive commands.
///
/// - `yes = true` → always Proceed (skip prompt)
/// - `yes = false` + non-TTY → Bail (refuse in non-interactive mode)
/// - `yes = false` + TTY + input "yes" → Proceed
/// - `yes = false` + TTY + other input → Cancel
fn evaluate_delete_confirmation(yes: bool, is_tty: bool, input: &str) -> DeleteConfirmation {
    if yes {
        return DeleteConfirmation::Proceed;
    }
    if !is_tty {
        return DeleteConfirmation::Bail;
    }
    if input.trim() == "yes" {
        DeleteConfirmation::Proceed
    } else {
        DeleteConfirmation::Cancel
    }
}
```

The `handle` dispatcher stays the same. Implement each handler following the `commands/models.rs` pattern:

```rust
async fn handle_list(json: bool) -> Result<()> {
    let mut client = connect_client().await?;
    let response = client
        .call(ControlRequest::new("provider.list", serde_json::json!({})))
        .await?;
    match response {
        ControlResponse::Ok(value) => {
            let resp: ProviderListResponseDto = serde_json::from_value(value)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&resp.providers)?);
            } else {
                print_providers_table(&resp.providers);
            }
            Ok(())
        }
        ControlResponse::Err(err) => {
            anyhow::bail!("RPC error [{}]: {}", err.code, err.message)
        }
    }
}

fn print_providers_table(providers: &[ProviderDto]) {
    if providers.is_empty() {
        println!("No providers found.");
        return;
    }
    let w_id = 10;
    let w_name = providers.iter().map(|p| p.name.len()).max().unwrap_or(4).max(4);
    let w_kind = 22;
    let w_url = providers.iter().map(|p| p.base_url.len()).max().unwrap_or(8).max(8);
    println!(
        "{:width_id$}  {:width_n$}  {:width_k$}  {:width_u$}  {:7}  {:5}",
        "ID", "NAME", "KIND", "BASE_URL", "ENABLED", "KEY",
        width_id = w_id, width_n = w_name, width_k = w_kind, width_u = w_url
    );
    for p in providers {
        let id_short = if p.id.len() > w_id { &p.id[..w_id] } else { &p.id };
        // `{:?}` on `ProviderKind::OpenAiCompatible` yields "OpenAiCompatible"
        // → "openaicompatible" (no underscore). Map to the wire string the GUI
        // and CLI flag parser both use.
        let kind_str = match p.provider_kind {
            ProviderKind::OpenAiCompatible => "openai_compatible",
            ProviderKind::AnthropicCompatible => "anthropic_compatible",
        };
        println!(
            "{:width_id$}  {:width_n$}  {:width_k$}  {:width_u$}  {:7}  {:5}",
            id_short, p.name, kind_str, p.base_url,
            if p.enabled { "yes" } else { "no" },
            if p.has_api_key { "yes" } else { "no" },
            width_id = w_id, width_n = w_name, width_k = w_kind, width_u = w_url
        );
    }
}

async fn handle_add(
    url: String,
    key: String,
    kind: String,
    name: Option<String>,
    model: Option<String>,
    tags: Option<String>,
) -> Result<()> {
    validate_base_url(&url)?;

    // Derive name (or use provided name). Collision-check against existing.
    let mut client = connect_client().await?;
    let list_resp = client
        .call(ControlRequest::new("provider.list", serde_json::json!({})))
        .await?;
    let existing_providers: ProviderListResponseDto = match list_resp {
        ControlResponse::Ok(v) => serde_json::from_value(v)?,
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    };
    let existing_names: std::collections::HashSet<String> = existing_providers
        .providers
        .iter()
        .map(|p| p.name.clone())
        .collect();

    let final_name = match name {
        Some(n) => n,
        None => derive_unique_provider_name(&url, &kind, &existing_names),
    };

    let parsed_kind = match kind.as_str() {
        "openai_compatible" => ProviderKind::OpenAiCompatible,
        "anthropic_compatible" => ProviderKind::AnthropicCompatible,
        other => anyhow::bail!("invalid kind: {other}"),
    };

    let create_req = ProviderCreateRequestDto {
        name: final_name.clone(),
        provider_kind: parsed_kind,
        base_url: url.clone(),
        api_key: Some(key),
        enabled: Some(true),
    };
    let provider: ProviderDto = {
        let resp = client
            .call(ControlRequest::new("provider.create", serde_json::to_value(&create_req)?))
            .await?;
        match resp {
            ControlResponse::Ok(v) => serde_json::from_value(v)?,
            ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
        }
    };
    println!("Created provider: {} ({})", provider.name, provider.id);

    // Optional sync model creation
    if let Some(model_name) = model {
        let model_tags = parse_tags(tags.as_deref().unwrap_or(""));
        let model_req = ModelCreateRequestDto {
            provider_id: provider.id.clone(),
            model_id: model_name.clone(),
            enabled: Some(true),
            tags: model_tags,
            context_window: 200000,
            max_tokens: 8192,
            display_name: Some(model_name.clone()),
            reasoning: Some(true),
        };
        let resp = client
            .call(ControlRequest::new("model.create", serde_json::to_value(&model_req)?))
            .await?;
        match resp {
            ControlResponse::Ok(_) => println!("Created model: {}", model_name),
            ControlResponse::Err(err) => anyhow::bail!(
                "Provider created, but model creation failed: RPC error [{}]: {}",
                err.code,
                err.message
            ),
        }
    }
    Ok(())
}

async fn handle_show(id: String) -> Result<()> {
    let mut client = connect_client().await?;
    let resp = client
        .call(ControlRequest::new("provider.list", serde_json::json!({})))
        .await?;
    let list: ProviderListResponseDto = match resp {
        ControlResponse::Ok(v) => serde_json::from_value(v)?,
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    };
    let provider = list
        .providers
        .into_iter()
        .find(|p| p.id == id)
        .with_context(|| format!("provider not found: {id}"))?;
    println!("{}", serde_json::to_string_pretty(&provider)?);
    Ok(())
}

async fn handle_update(
    id: String,
    name: Option<String>,
    url: Option<String>,
    key: Option<String>,
    kind: Option<String>,
    enabled: Option<bool>,
) -> Result<()> {
    if let Some(ref u) = url {
        validate_base_url(u)?;
    }
    let provider_kind = match kind.as_deref() {
        Some("openai_compatible") => Some(ProviderKind::OpenAiCompatible),
        Some("anthropic_compatible") => Some(ProviderKind::AnthropicCompatible),
        Some(other) => anyhow::bail!("invalid kind: {other}"),
        None => None,
    };
    let req = ProviderUpdateRequestDto {
        id: id.clone(),
        name,
        base_url: url,
        enabled,
        provider_kind,
        api_key: key.map(Some), // Some(Some(k)) = update; None = no change
    };
    let mut client = connect_client().await?;
    let resp = client
        .call(ControlRequest::new("provider.update", serde_json::to_value(&req)?))
        .await?;
    match resp {
        ControlResponse::Ok(v) => {
            let updated: ProviderDto = serde_json::from_value(v)?;
            println!("Updated provider: {} ({})", updated.name, updated.id);
            Ok(())
        }
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    }
}

async fn handle_delete(id: String, yes: bool) -> Result<()> {
    if !yes {
        let is_tty = std::io::stdin().is_terminal();
        let input = if is_tty {
            println!("Delete provider {} and all its models?", id);
            println!("Note: bound subagents will fail on next delegate. Rebind manually.");
            print!("Type 'yes' to confirm: ");
            use std::io::Write;
            std::io::stdout().flush()?;
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line
        } else {
            String::new()
        };
        match evaluate_delete_confirmation(yes, is_tty, &input) {
            DeleteConfirmation::Proceed => {}
            DeleteConfirmation::Cancel => {
                println!("Cancelled.");
                return Ok(());
            }
            DeleteConfirmation::Bail => {
                anyhow::bail!("Refusing to delete in non-interactive mode without --yes");
            }
        }
    }
    let mut client = connect_client().await?;
    let resp = client
        .call(ControlRequest::new("provider.delete", serde_json::json!({ "id": id })))
        .await?;
    match resp {
        ControlResponse::Ok(_) => {
            println!("Deleted provider: {}", id);
            Ok(())
        }
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    }
}

async fn handle_test(id: String) -> Result<()> {
    let mut client = connect_client().await?;
    let resp = client
        .call(ControlRequest::new("provider.test_connection", serde_json::json!({ "id": id })))
        .await?;
    match resp {
        ControlResponse::Ok(v) => {
            let result: ProviderTestConnectionResponseDto = serde_json::from_value(v)?;
            if result.ok {
                println!("✓ connection ok");
                if let Some(models) = result.models_detected {
                    println!("  detected {} models", models.len());
                }
            } else {
                println!("✗ connection failed: {}", result.error.unwrap_or_default());
            }
            Ok(())
        }
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    }
}

async fn handle_model(subcommand: ProviderModelCommand) -> Result<()> {
    match subcommand {
        ProviderModelCommand::List { provider_id, json } => {
            handle_model_list(provider_id, json).await
        }
        ProviderModelCommand::Add {
            provider_id, name, tags, context_window, max_tokens, reasoning, display_name,
        } => {
            handle_model_add(provider_id, name, tags, context_window, max_tokens, reasoning, display_name).await
        }
        ProviderModelCommand::Update {
            provider_id, model_id, tags, context_window, max_tokens, reasoning, enabled, display_name,
        } => {
            handle_model_update(provider_id, model_id, tags, context_window, max_tokens, reasoning, enabled, display_name).await
        }
        ProviderModelCommand::Delete { provider_id, model_id, yes } => {
            handle_model_delete(provider_id, model_id, yes).await
        }
    }
}

async fn handle_model_list(provider_id: String, json: bool) -> Result<()> {
    let req = ModelListRequestDto {
        provider_id: Some(provider_id),
        tags: vec![],
        include_disabled: true,
    };
    let mut client = connect_client().await?;
    let resp = client
        .call(ControlRequest::new("model.list", serde_json::to_value(&req)?))
        .await?;
    match resp {
        ControlResponse::Ok(v) => {
            let list: ModelListResponseDto = serde_json::from_value(v)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list.models)?);
            } else {
                print_models_table(&list.models);
            }
            Ok(())
        }
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    }
}

fn print_models_table(models: &[busytok_protocol::dto::ModelCatalogEntryDto]) {
    if models.is_empty() {
        println!("No models found.");
        return;
    }
    let w_id = models.iter().map(|m| m.model_id.len()).max().unwrap_or(5).max(5);
    let w_tags = 20;
    println!(
        "{:width_m$}  {:6}  {:width_t$}",
        "MODEL", "ENABLE", "TAGS",
        width_m = w_id, width_t = w_tags
    );
    for m in models {
        let tags = m.tags.join(",");
        let en = if m.model_enabled { "yes" } else { "no" };
        println!(
            "{:width_m$}  {:6}  {:width_t$}",
            m.model_id, en, tags,
            width_m = w_id, width_t = w_tags
        );
    }
}

async fn handle_model_add(
    provider_id: String,
    name: String,
    tags: Option<String>,
    context_window: Option<i64>,
    max_tokens: Option<i64>,
    reasoning: bool,
    display_name: Option<String>,
) -> Result<()> {
    let model_tags = parse_tags(tags.as_deref().unwrap_or(""));
    let req = ModelCreateRequestDto {
        provider_id: provider_id.clone(),
        model_id: name.clone(),
        enabled: Some(true),
        tags: model_tags,
        context_window: context_window.unwrap_or(200000),
        max_tokens: max_tokens.unwrap_or(8192),
        display_name: Some(display_name.unwrap_or_else(|| name.clone())),
        reasoning: Some(reasoning),
    };
    let mut client = connect_client().await?;
    let resp = client
        .call(ControlRequest::new("model.create", serde_json::to_value(&req)?))
        .await?;
    match resp {
        ControlResponse::Ok(_) => {
            println!("Created model: {} under provider: {}", name, provider_id);
            Ok(())
        }
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    }
}

async fn handle_model_update(
    provider_id: String,
    model_id: String,
    tags: Option<String>,
    context_window: Option<i64>,
    max_tokens: Option<i64>,
    reasoning: Option<bool>,
    enabled: Option<bool>,
    display_name: Option<String>,
) -> Result<()> {
    // Resolve model_db_id via model.list (include_disabled: true)
    let list_req = ModelListRequestDto {
        provider_id: Some(provider_id.clone()),
        tags: vec![],
        include_disabled: true,
    };
    let mut client = connect_client().await?;
    let list_resp = client
        .call(ControlRequest::new("model.list", serde_json::to_value(&list_req)?))
        .await?;
    let list: ModelListResponseDto = match list_resp {
        ControlResponse::Ok(v) => serde_json::from_value(v)?,
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    };
    let model = list
        .models
        .into_iter()
        .find(|m| m.model_id == model_id)
        .with_context(|| format!("model not found: {model_id} under provider {provider_id}"))?;

    let update_req = ModelUpdateRequestDto {
        id: model.model_db_id.clone(),
        enabled,
        display_name,
        reasoning,
        context_window,
        max_tokens,
    };
    let resp = client
        .call(ControlRequest::new("model.update", serde_json::to_value(&update_req)?))
        .await?;
    match resp {
        ControlResponse::Ok(_) => {}
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    }

    // Tags are updated via a separate RPC
    if let Some(tags_str) = tags {
        let parsed_tags = parse_tags(&tags_str);
        let tags_resp = client
            .call(ControlRequest::new(
                "model.tags.update",
                serde_json::json!({ "model_id": model.model_db_id, "tags": parsed_tags }),
            ))
            .await?;
        match tags_resp {
            ControlResponse::Ok(_) => {}
            ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
        }
    }
    println!("Updated model: {}", model_id);
    Ok(())
}

async fn handle_model_delete(provider_id: String, model_id: String, yes: bool) -> Result<()> {
    if !yes {
        let is_tty = std::io::stdin().is_terminal();
        let input = if is_tty {
            println!("Delete model {} under provider {}?", model_id, provider_id);
            println!("Note: bound subagents will fail on next delegate.");
            print!("Type 'yes' to confirm: ");
            use std::io::Write;
            std::io::stdout().flush()?;
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line
        } else {
            String::new()
        };
        match evaluate_delete_confirmation(yes, is_tty, &input) {
            DeleteConfirmation::Proceed => {}
            DeleteConfirmation::Cancel => {
                println!("Cancelled.");
                return Ok(());
            }
            DeleteConfirmation::Bail => {
                anyhow::bail!("Refusing to delete in non-interactive mode without --yes");
            }
        }
    }
    // Resolve model_db_id
    let list_req = ModelListRequestDto {
        provider_id: Some(provider_id.clone()),
        tags: vec![],
        include_disabled: true,
    };
    let mut client = connect_client().await?;
    let list_resp = client
        .call(ControlRequest::new("model.list", serde_json::to_value(&list_req)?))
        .await?;
    let list: ModelListResponseDto = match list_resp {
        ControlResponse::Ok(v) => serde_json::from_value(v)?,
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    };
    let model = list
        .models
        .into_iter()
        .find(|m| m.model_id == model_id)
        .with_context(|| format!("model not found: {model_id} under provider {provider_id}"))?;

    let resp = client
        .call(ControlRequest::new(
            "model.delete",
            serde_json::json!({ "id": model.model_db_id }),
        ))
        .await?;
    match resp {
        ControlResponse::Ok(_) => {
            println!("Deleted model: {}", model_id);
            Ok(())
        }
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    }
}
```

**Important:** The `ProvidersRuntime` test wrapper at the bottom of the file must implement ALL ~35 `RuntimeControl` trait methods, delegating each to `self.inner.<method>(req).await` except for the ones the test overrides (`provider_list` in the example). Copy the delegation boilerplate verbatim from `apps/cli/src/commands/models.rs:288-533`, changing the struct name to `ProvidersRuntime` and overriding `provider_list` (and `provider_create`, `provider_update`, `provider_delete`, `provider_test_connection`, `model_list`, `model_create`, `model_update`, `model_delete` as needed per test case).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p busytok-cli --bin busytok -- commands::provider`
Expected: PASS — all tests pass.

- [ ] **Step 5: Run full CLI test suite + clippy + fmt**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/cli/src/commands/provider.rs
git commit -m "feat(cli): implement provider and model handlers

List/Add/Show/Update/Delete/Test for providers; List/Add/Update/Delete
for models. Name derivation + collision check mirror the GUI helpers.
Delete commands gate on --yes or TTY prompt (non-interactive mode
requires --yes). model.list for update/delete resolution uses
include_disabled: true."
```

---

## Self-Review

### 1. Spec coverage

| Spec section | Covered by task(s) |
|---|---|
| §1 Goal: Delete Profiles UI | Task 1 |
| §1 Goal: Simplify Provider creation form | Task 3 (utils) + Task 6 (form) |
| §1 Goal: Redesign Provider display as cards | Task 4 (card) + Task 8 (page rewrite) |
| §1 Goal: Simplify Model creation form | Task 5 (inline model CRUD) |
| §1 Goal: Provider header inline-edit | Task 7 |
| §1 Goal: CLI `busytok provider` subcommand group | Task 9 (enum) + Task 10 (handlers) |
| §2 Delete Profiles UI (files + modifications) | Task 1 |
| §3 Provider Creation Form (fields, name derivation, collision, validation, partial-success) | Task 3 (utils) + Task 6 (form) |
| §4 Provider Card (layout, view mode, edit mode, delete confirm, test connection) | Task 4 (view) + Task 7 (edit) + Task 8 (page) |
| §5 Model Creation/Edit (fields, advanced, inline edit, delete) | Task 5 |
| §6 CSS / Styling (classes + tokens) | Task 2 |
| §7 CLI Provider Commands (enum, dispatch, handlers, name gen, output) | Task 9 + Task 10 |
| §8 Observability (frontend events) | Distributed: Task 5 (model.added/updated/deleted), Task 6 (provider.added + partial-success), Task 7 (provider.updated), Task 8 (provider.deleted/tested/test.failed + model.* on page) |
| §9 File Impact Summary | Maps to File Structure section |
| §10 Testing | Each task has TDD steps with concrete test code |
| §11 Constraints | Global Constraints section (verbatim values) |

No gaps — every spec section maps to at least one task. **Fixes applied during review:**
1. Task 8's `handleTestConnection` `onError` handler was emitting `provider.tested` (INFO); corrected to `provider.test.failed` (ERROR) per spec §8 (client-side exception path). Added corresponding test case.
2. Task 8's `mockModelMutations` `createModel` mock called `opts?.onSuccess?.()` with no argument, but the page's `handleModelCreate` `onSuccess` reads `entry.provider_id` + `entry.model_id` — would have thrown a `TypeError`. Fixed the mock to pass a concrete `ModelCatalogEntryDto` and corrected the `onSuccess` callback type signature from `() => void` to `(entry: ModelCatalogEntryDto) => void`.
3. Task 6's `ProviderCreationFormProps` declared an `onProviderCreated?` callback that was never used in implementation (Task 6) or consumption (Task 8). Removed the dead prop and added a rationale note (the page observes changes via TanStack Query refetch, not a callback).
4. Task 10's `print_providers_table` used `format!("{:?}", p.provider_kind).to_lowercase()` which produces `"openaicompatible"` (no underscore) for `ProviderKind::OpenAiCompatible`. Replaced with an explicit `match` returning the wire string `"openai_compatible"` / `"anthropic_compatible"` that the GUI and CLI flag parser both use.
5. Task 10's `ProvidersRuntime` test wrapper used `// ... (other delegation methods would be needed; elided here for brevity)` placeholder for the `RuntimeControl` trait impl. Expanded with the full ~35-method delegation boilerplate (verbatim from `commands/models.rs:288-533` pattern) so the plan contains no elided code. Updated the "Note on the ProvidersRuntime wrapper" accordingly.

### 2. Placeholder scan

Scanned for: "TBD", "TODO", "implement later", "fill in details", "Add appropriate error handling", "add validation", "handle edge cases", "Write tests for the above" (without test code), "Similar to Task N", "elided here for brevity", `// ...`. **One found and fixed:** Task 10's `ProvidersRuntime` trait impl used `// ... (other delegation methods would be needed; elided here for brevity)` — replaced with the complete delegation boilerplate (see fix #5 above). After the fix, no placeholders remain — every step contains concrete code or exact commands.

### 3. Type consistency

Cross-task type references verified:
- `deriveProviderName(url: string, kind: string): string` — defined Task 3, used Task 6; mirrored in Rust Task 10 as `derive_provider_name(url, kind) -> Option<String>` ✓
- `deriveUniqueProviderName(url, kind, existingNames: Set<string>): string` — defined Task 3, used Task 6 ✓
- `validateBaseUrl(input: string): string | null` — defined Task 3, used Task 6; CLI mirror `validate_base_url(input) -> Result<()>` in Task 10 ✓
- `parseTags(input: string): string[]` — defined Task 3, used Task 5 + Task 6 ✓
- `ProviderCard` props (mutations + models + test connection) — defined Task 4, consumed Task 8 ✓
- `ProviderCreationForm` props — defined Task 6 (now `onClose` only after dead-prop removal), consumed Task 8 ✓
- `ProviderUpdateRequestDto.api_key` three-state `Option<Option<String>>` — Task 7 handles "empty = no change (None), typed = update (Some(Some(v)))" ✓
- `ModelUpdateRequestDto` single-state `Option<T>` — Task 5 treats empty input as "leave unchanged" ✓
- `include_disabled: true` — present in Task 10's model.list for update + delete resolution, matching spec §7 ✓
- `createModel` mutation return type — `ModelCatalogEntryDto` per `useBusytokData.ts:470`; Task 8 `handleModelCreate` `onSuccess` reads `entry.provider_id` + `entry.model_id`, and the test mock now passes a concrete entry (see fix #2) ✓
- `model.tags.update` wire field — `model_id: String` per `ModelTagUpdateDto` (dto.rs:1720); the value sent is the SQL PK (`model_db_id`), matching existing `ModelsSection.tsx:350` (`tagsUpdate.mutate({ modelId: model.model_db_id, ... })`). Task 10 `handle_model_update` sends `model.model_db_id` ✓
- `ProviderKind` formatting in Task 10 `print_providers_table` — now uses explicit `match` returning wire strings, matching `clap` `value_parser` and GUI selector ✓

All types and signatures are consistent across tasks and the actual codebase.

### 4. Second-round review fixes (user-reported findings)

Five additional issues found by the user and fixed:

1. **P1: Model inline edit not implemented (Task 5)** — The `编辑` button on each model row was an empty `<button>` with no `onClick`; `onModelUpdate` / `onModelTagsUpdate` were declared in props but never destructured or called; no tests covered the edit flow. **Fixed:** Added `ModelEditDraft` interface, `editingModelDbId` state, `toEditDraft` helper, `startModelEdit` / `cancelModelEdit` / `handleEditSubmit` handlers. The edit form renders Display Name, Tags, Context Window, Max Tokens, Reasoning, Enabled inputs. `handleEditSubmit` builds a `ModelUpdateRequestDto` patch with only changed fields (single-state `Option<T>` semantics) and calls `onModelUpdate`; if tags changed, calls `onModelTagsUpdate` with parsed tags. Added 5 edit tests in Task 5 + 4 event tests in Task 8 (`model.updated`, `model.update.failed`, `model.tags.updated`, `model.tags.update.failed`). Also fixed `onModelUpdate` signature from `(modelDbId: string, patch)` to `(model: ModelCatalogEntryDto, patch)` so the page can emit spec-§8-correct details `{ provider_id, model_id }` instead of the wrong `{ model_db_id }`.

2. **P1: GUI verification commands not executable (all tasks)** — Plan used `pnpm --filter gui ...` but the package name is `@busytok/gui`. Also referenced `pnpm --filter gui lint` but `apps/gui/package.json` has no `lint` script. Plan said "coverage ≥90%" but never ran the repo's `pnpm coverage:gui` gate. **Fixed:** Replaced all `pnpm --filter gui` → `pnpm --filter @busytok/gui`. Replaced `lint` with `typecheck` (which exists). Added `pnpm coverage:gui` to the Global Constraints and to Task 8's final gate. Root `package.json` confirms: `coverage:gui` = `pnpm --filter @busytok/gui test -- --coverage.enabled true --coverage.thresholds.lines 90`.

3. **P1: CLI handler won't compile (Task 10)** — `use busytok_protocol::dto::{... ProviderKind ...}` would fail: `ProviderKind` is defined in `busytok_domain` and is NOT re-exported from `busytok_protocol::dto`. The test module already imported it correctly from `busytok_domain`. **Fixed:** Handler imports now use `use busytok_domain::ProviderKind;` separately, and `ProviderKind` was removed from the `busytok_protocol::dto::{...}` import list. Updated the Interfaces section to document this.

4. **P1: ProviderCard editDraft setState during render (Task 7)** — `if (isEditing && editDraft === null) { setEditDraft(...) }` and `if (!isEditing && editDraft !== null) { setEditDraft(null) }` both call `setState` during the render phase, which is unstable in React 19 (especially under StrictMode and React Compiler). **Fixed:** Replaced with a single `useEffect` dependent on `[isEditing, provider.id, provider.name, provider.base_url, provider.provider_kind]` that initializes the edit draft when entering edit mode and clears it when exiting. Added `useEffect` to the import list.

5. **P2: CLI destructive safety test didn't assert safety semantics (Task 10)** — `handle_delete_bails_without_yes_in_non_tty` called `handle_delete` and discarded the result with `let _ = result;` — it didn't actually verify that non-interactive mode without `--yes` bails. **Fixed:** Extracted a pure `evaluate_delete_confirmation(yes, is_tty, input) -> DeleteConfirmation` function (returns `Proceed` / `Cancel` / `Bail`) and a `DeleteConfirmation` enum. Refactored `handle_delete` and `handle_model_delete` to use it. Replaced the weak test with 4 deterministic unit tests: `confirmation_proceeds_with_yes_flag`, `confirmation_bails_in_non_tty_without_yes`, `confirmation_proceeds_in_tty_with_yes_input`, `confirmation_cancels_in_tty_with_non_yes_input`. Also kept an integration test `handle_delete_proceeds_with_yes_flag` that verifies the full handler proceeds with `--yes`.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-06-provider-page-redesign.md`.

**Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration. Best for this plan: 10 tasks with clear boundaries, TDD cycles benefit from review gates.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints. Heavier on the main context but no handoff overhead.