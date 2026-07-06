# Provider Page Redesign — Design Spec

Date: 2026-07-06
Status: Approved (pending review)

## 1. Goal

Redesign the Provider page UI for simplicity and clarity, delete the Profiles section, streamline provider/model creation flows, and add CLI parity for all provider+model CRUD operations.

### Scope

**In scope:**
- Delete Profiles UI (frontend only; backend RPCs stay)
- Simplify Provider creation form (3 required fields + optional inline model)
- Redesign Provider display as always-expanded cards with inline model list
- Simplify Model creation form (name required, tags optional; metadata fields as optional advanced)
- Provider header inline-edit (models stay visible, operations disabled during edit)
- CLI `busytok provider` subcommand group with full CRUD + model management

**Out of scope:**
- Backend RPC changes (all existing `provider.*` / `model.*` RPCs are reused as-is)
- Profiles backend removal (RPCs remain; only frontend is removed)
- Analytics/usage pages
- Subagent binding UI

---

## 2. Delete Profiles UI

### Files to delete
- `apps/gui/src/components/ProfilesSection.tsx`
- `apps/gui/src/components/ProfilesSection.test.tsx`

### Files to modify

**`apps/gui/src/pages/ProvidersPage.tsx`:**
- Remove `import { ProfilesSection }` (line 12)
- Remove `<ProfilesSection />` render (line 703)

**`apps/gui/src/api/useBusytokData.ts`:**
- Delete `useProfileMutations` hook (lines 490-517)
- Delete `ProfileCreateRequestDto` / `ProfileUpdateRequestDto` imports (lines 45-46)

**`apps/gui/src/api/busytokClient.ts`:**
- Delete `profileCreate`, `profileUpdate`, `profileDelete` methods (lines 210-215)
- Delete `ProfileCreateRequestDto`, `ProfileUpdateRequestDto`, `ProfileDto` imports (lines 44-46). `ProfileDto` is unused after removing the profile methods; the generated `SettingsSubagentDto` in `@busytok/protocol-types` still carries `ProfileDto` but that's a separate package, not this import.

### Preserve
- `useSettingsSnapshot` hook — shared by other settings UI
- `ProfileDto` type — embedded in generated `SettingsSubagentDto`, cannot remove without backend schema change
- Backend `profile.create` / `profile.update` / `profile.delete` RPCs — out of scope

---

## 3. Provider Creation Form

### Fields

| Field | Type | Required | Default | Notes |
|-------|------|----------|---------|-------|
| Base URL | text | yes | — | Must start with `http://` or `https://` |
| API Key | password | yes | — | Sent as string to backend |
| Kind | select | yes | `openai_compatible` | `openai_compatible` / `anthropic_compatible` |
| Model Name | text | no | — | Filled → sync-create a model after provider is created |
| Model Tags | text | no | `[]` | Comma-separated, parsed to `string[]` |

### DisplayName auto-generation

Provider `name` is not a form field — it is auto-derived from Base URL + Kind:

```typescript
function deriveProviderName(url: string, kind: string): string {
  const host = new URL(url).hostname;                    // "api.deepseek.com"
  const parts = host.split(".");
  const domain = parts[parts.length - 2] || host;        // "deepseek"
  const kindShort = kind.replace("_compatible", "");     // "openai"
  return `${domain}_${kindShort}`;                       // "deepseek_openai"
}
```

**Edge cases:**
- `https://localhost:8080/v1` → `parts = ["localhost"]`, `parts[-2]` is undefined → falls back to `|| host` → `"localhost_openai"`. Acceptable.
- `https://api.co.uk/v1` → `parts[-2]` = `"co"` → `"co_openai"`. Wrong but acceptable for v1 (rare case for API endpoints). Users can always edit the name afterward.

**Collision handling:** Before submitting, check the derived name against existing provider names. If collision, append `_2`, `_3`, etc. until unique:

```typescript
function deriveUniqueProviderName(
  url: string,
  kind: string,
  existingNames: Set<string>
): string {
  const base = deriveProviderName(url, kind);
  if (!existingNames.has(base)) return base;
  let i = 2;
  while (existingNames.has(`${base}_${i}`)) i++;
  return `${base}_${i}`;
}
```

The `existingNames` set is built from the `useProviders()` query result.

### URL validation

Before deriving the name or submitting, validate the Base URL:

```typescript
function validateBaseUrl(input: string): string | null {
  const trimmed = input.trim();
  if (!trimmed) return "Base URL 不能为空";
  if (!/^https?:\/\//.test(trimmed))
    return "请输入完整的 URL（以 http:// 或 https:// 开头）";
  try {
    new URL(trimmed);
  } catch {
    return "URL 格式不正确";
  }
  return null; // valid
}
```

Validation runs on blur and on submit. Error message shown below the input field.

### Submit flow

1. Validate Base URL + API Key + Kind
2. Derive unique provider name from existing providers list
3. Call `provider.create` with `{ name, provider_kind, base_url, api_key, enabled: true }`
4. If Model Name is non-empty: call `model.create` with `{ provider_id, model_id, display_name: <model_id>, context_window: 200000, max_tokens: 8192, reasoning: true, enabled: true, tags: parseTags(tagsInput) }` (display_name sent explicitly as model_id because the backend stores NULL without fallback)
5. **Full success (provider + optional model both succeed):** close form, invalidate `providers` + `models` queries, report `provider.added` (+ `model.added` if model was created)
6. **Partial success (provider created, model failed):** invalidate `providers` query (the new provider must appear in the list), keep the form open with a recoverable error banner showing the model creation failure message, disable the Save button to prevent duplicate provider creation. The user can either: (a) fix the model name and retry only `model.create`, or (b) click Cancel to dismiss the form — the provider is already saved and visible in the list. The form state should track that `provider.create` already succeeded so a retry skips it and only re-attempts `model.create`.
7. **Provider creation failed:** keep form open, show error message, do not invalidate any queries.

### Form layout

Vertical stack within a `settings-panel` card:
1. Base URL input
2. API Key input (password type)
3. Kind select dropdown
4. Divider
5. "同步创建 Model" label (always visible, not a collapsible)
6. Model Name input
7. Model Tags input
8. Save / Cancel buttons

---

## 4. Provider Card (Always-Expanded)

### Layout

```
┌──────────────────────────────────────────────────────────┐
│  deepseek_openai    [openai]    [● enabled]   [✏][🔌][🗑] │  Header
│  https://api.deepseek.com/v1          ID: abc-123         │  Info
├──────────────────────────────────────────────────────────┤
│  Models                              [+ Add Model]        │  Sub-header
│  ┌────────────────────────────────────────────────────┐  │
│  │ deepseek-chat   [●enabled]  tags: cheap,fast  [✏][🗑]│  │  Model row
│  │ deepseek-reason [●enabled]  tags: reasoning   [✏][🗑]│  │  Model row
│  └────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────┘
```

### View mode (default)

**Header row:** Provider name (bold) + Kind chip + Enabled toggle + action buttons (Edit / Test Connection / Delete)

**Info row:** Base URL + Provider ID (read-only, monospace)

**Models section:**
- "Models" label + "+ Add Model" button on the right
- Models are fetched via a single `useModels({ includeDisabled: true })` call (all providers, no provider filter, include disabled models so the enabled toggle can re-enable them) and grouped client-side by `provider_id` into a `Map<provider_id, ModelCatalogEntryDto[]>`. Each card renders its models from this map. This is 1 query vs N-per-card.
- Each model is a row: model_id + enabled toggle + tags (chips) + Edit / Delete buttons
- Clicking Edit on a model row expands it inline to show editable fields
- Clicking "+ Add Model" shows a new model row at the bottom with inline form

### Edit mode (provider header inline-edit)

When user clicks Edit on the provider:
- **Header row** fields become inputs: Base URL (text), API Key (password, with "Update Key" label), Kind (select)
- **Info row** shows Provider ID (read-only) + Provider Name (editable text input)
- **API Key convention:** empty input = no change (omit from patch). To clear the key, user must use a separate "Clear Key" action (not in v1 scope). Typing a new value = update key.
- **Save / Cancel** buttons replace the action buttons in the header
- **Models section stays visible** but:
  - All model operation buttons (Add Model, Edit, Delete) are `disabled` with reduced opacity
  - A notice appears: "正在编辑 Provider 信息，Models 操作暂不可用"
- On Save: call `provider.update` with changed fields, return to view mode
- On Cancel: discard changes, return to view mode

### Delete confirmation

Provider delete shows `confirm("确定删除此 provider 及其关联的所有 models？\n注意：已绑定此 provider/model 的 subagents 将在下次 delegate 时失败，需要手动重新绑定。")`. On confirm, call `provider.delete`. The backend cascades model deletion via FK `ON DELETE CASCADE`, but `subagent_logical_subagents.bound_provider_id` / `bound_model_id` are NOT FK-constrained — they become dangling references. No automatic rebind is performed. Users must update affected subagents' bindings manually.

Model delete shows `confirm("确定删除此 model？\n注意：已绑定此 model 的 subagents 将在下次 delegate 时失败。")` with the same dangling-reference caveat.

### Test connection

Same as current — calls `provider.test_connection`, shows result in a temporary toast/inline message below the header.

---

## 5. Model Creation / Edit (Simplified)

### Model creation fields

| Field | Required | Default | Notes |
|-------|----------|---------|-------|
| Model Name | yes | — | e.g. `deepseek-chat` |
| Tags | no | `[]` | Comma-separated |
| Context Window | no | 200000 | Advanced, collapsed by default |
| Max Tokens | no | 8192 | Advanced, collapsed by default |
| Reasoning | no | true | Checkbox, default checked |
| Display Name | no | = Model Name | Advanced, collapsed by default |

### Advanced fields

The form has an expandable "高级设置" section (default collapsed) containing:
- Display Name (text, placeholder: defaults to Model Name)
- Context Window (number, placeholder: 200000)
- Max Tokens (number, placeholder: 8192)
- Reasoning (checkbox, default checked)

If user leaves advanced fields empty, the submit payload uses:
- `display_name: <model_id>` (sent explicitly — backend stores NULL without fallback to model_id)
- `context_window: 200000`
- `max_tokens: 8192`
- `reasoning: true`
- `enabled: true`
- `tags: parseTags(tagsInput)` (empty array if no tags entered)

### Model inline edit

Clicking Edit on a model row expands it to show:
- Model Name (read-only — this is the ID, immutable)
- Display Name (text)
- Tags (text, comma-separated)
- Context Window (number)
- Max Tokens (number)
- Reasoning (checkbox)
- Enabled (toggle)
- Save / Cancel buttons

On Save: call `model.update` with changed fields. If tags changed, call `model.tags.update`. **Note:** `ModelUpdateRequestDto` fields are single-state `Option<T>` — omit = no change, `Some(v)` = set. There is no way to clear a field to null. Empty `display_name` input means "leave unchanged", not "clear".

### Model delete

`confirm("确定删除此 model？")` → `model.delete`.

---

## 6. CSS / Styling

Reuse the existing custom CSS design system. Add new classes:

```css
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
.chip {
  display: inline-flex;
  align-items: center;
  padding: 2px 8px;
  border-radius: 4px;
  font-size: 0.75rem;
  background: var(--color-surface-subtle);
  color: var(--color-text-muted);
}
```

No Tailwind, no external CSS framework. All styles go in `apps/gui/src/styles/pages.css` or `components.css`.

### Design token reference

The classes above use the existing token system defined in `apps/gui/src/styles/tokens.css` (light + dark variants). Do **not** introduce parallel tokens like `--border-color`, `--surface-bg`, `--text-muted`, `--hover-bg`, `--chip-bg`, or `--chip-text` — they do not exist and would silently fall back to `unset`.

| Token used here | Light value | Purpose |
|-----------------|-------------|---------|
| `--color-border-subtle` | `rgba(15, 23, 42, 0.07)` | Card / header dividers |
| `--color-surface` | `#FFFFFF` | Card background |
| `--color-surface-subtle` | `#F7F8FA` | Chip background |
| `--color-text` | `#1A1D23` | Primary text (provider name) |
| `--color-text-muted` | `#6b7280` | Secondary text + chip text |
| `--color-hover` | `rgba(15, 23, 42, 0.04)` | Model row hover |

For status colors (enabled toggle, error banners, test-connection result), use the `--color-status-success` / `--color-status-warning` / `--color-status-danger` (+ `-soft`) family already defined in `tokens.css`.

---

## 7. CLI Provider Commands

### Command structure

New `ProviderCommand` enum in `apps/cli/src/main.rs`:

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
    /// List models for a provider
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

### Dispatch wiring

In `main.rs`, add to `Command` enum:
```rust
/// Manage providers and their models
Provider {
    #[command(subcommand)]
    subcommand: ProviderCommand,
},
```

In the match arm:
```rust
Command::Provider { subcommand } => {
    commands::provider::handle(subcommand).await
}
```

### Handler module

New file `apps/cli/src/commands/provider.rs`:

- `handle(cmd: ProviderCommand) -> Result<()>` — dispatch
- Each subcommand handler:
  - `handle_list(json: bool)` — call `provider.list`, print table or JSON
  - `handle_add(url, key, kind, name, model, tags)` — derive name if not given, call `provider.create`, optionally call `model.create` with defaults (`context_window: 200000, max_tokens: 8192, reasoning: true, display_name: <model_id>, enabled: true, tags: parseTags(tags)`)
  - `handle_show(id)` — call `provider.list`, find by id, print detail
  - `handle_update(id, name, url, key, kind, enabled)` — build patch DTO, call `provider.update`
  - `handle_delete(id, yes)` — if `yes` is false and stdin is a TTY, prompt for confirmation; if `yes` is false and stdin is NOT a TTY, bail with error ("use --yes to skip confirmation in non-interactive mode"); if `yes` is true, skip prompt. Then call `provider.delete`.
  - `handle_test(id)` — call `provider.test_connection`, print result
  - `handle_model_list(provider_id, json)` — call `model.list` with `{ provider_id, include_disabled: true }`. `include_disabled` is mandatory here: `ModelListRequestDto` defaults it to `false` [dto.rs](file:///Users/wsd/Data/Busytok/busytok/crates/busytok-protocol/src/dto.rs), so without it disabled models would disappear from CLI management — contradicting the UI which lets users re-enable them.
  - `handle_model_add(provider_id, name, tags, context_window, max_tokens, reasoning, display_name)` — call `model.create` with defaults: `context_window.unwrap_or(200000)`, `max_tokens.unwrap_or(8192)`, `display_name.unwrap_or(name.clone())`, `enabled: true`, `tags: parseTags(tags)`. Tags are parsed from comma-separated string: `tags.unwrap_or("").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()`.
  - `handle_model_update(provider_id, model_id, ...)` — first call `model.list` with `{ provider_id, include_disabled: true }` to resolve the internal DB UUID (`model_db_id`) from the user-facing `model_id` string, then call `model.update` with `id: model_db_id`. Call `model.tags.update` if tags changed. **Note:** `ModelUpdateRequestDto` fields are single-state `Option<T>` (omit = no change, `Some(v)` = set); there is no way to clear a field to null. The edit form must treat empty `display_name` as "leave unchanged" rather than "clear".
  - `handle_model_delete(provider_id, model_id)` — resolve `model_db_id` via `model.list` with `{ provider_id, include_disabled: true }` (same as update; a disabled model is still deletable), confirm, call `model.delete` with `{ id: model_db_id }`

### CLI name auto-generation

Same logic as UI — `derive_provider_name(url, kind)` in Rust. No `url` crate dependency; manual host extraction. The CLI `handle_add` validates the URL before calling this — checks for `://` prefix and non-empty host:

```rust
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
```
```rust
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
```

Collision check: call `provider.list` first, check existing names, append suffix if needed.

### Output format

**Text (default):**
- `provider list` — table with columns: ID (truncated), Name, Kind, Base URL, Enabled, Models count
- `provider show` — detail card with provider fields + model list
- `provider model list` — table with columns: Model ID, Display Name, Tags, Enabled

**JSON (`--json`):**
- Raw DTO JSON, pretty-printed

### Existing `busytok models` command

The existing `Command::Models` (read-only list, line 122) is kept unchanged for backward compatibility. It does the same thing as `provider model list` but without requiring a provider_id. No deprecation needed — just documented as a legacy convenience alias.

---

## 8. Observability (Frontend Events)

The current `ProvidersPage` and `ModelsSection` emit frontend events via `reportFrontendEventSafely` (see [safeReporter.ts](file:///Users/wsd/Data/Busytok/busytok/apps/gui/src/logging/safeReporter.ts)). Since this spec deletes `ModelsSection` and rewrites `ProvidersPage`, the rewrite **must preserve equivalent events** so observability does not regress silently. The convention (established by `ProfilesSection`) is: success emits a past-tense `*.added` / `*.updated` / `*.deleted` INFO event; failure emits a `*.<verb>.failed` ERROR event.

### Events to emit

| Operation | Success event (INFO) | Failure event (ERROR) |
|-----------|----------------------|------------------------|
| Provider create | `provider.added` | `provider.add.failed` |
| Provider update (inline edit) | `provider.updated` | `provider.update.failed` |
| Provider delete | `provider.deleted` | `provider.delete.failed` |
| Provider test connection | `provider.tested` (with `ok` + `error` in details) | `provider.test.failed` (on client-side exception only; RPC-returned `ok: false` still uses `provider.tested` to preserve current semantics) |
| Model create (inline / sync) | `model.added` | `model.add.failed` |
| Model update (inline edit) | `model.updated` | `model.update.failed` |
| Model tags update | `model.tags.updated` | `model.tags.update.failed` |
| Model delete | `model.deleted` | `model.delete.failed` |

### Details payloads

- `provider.added` / `provider.add.failed`: `{ name }` (the derived name; on partial-success retry, do not double-emit `provider.added` — track that it already succeeded)
- `provider.updated` / `provider.update.failed`: `{ id, name }`
- `provider.deleted` / `provider.delete.failed`: `{ id, name }`
- `provider.tested`: `{ id, ok, error }` (matches current shape in [ProvidersPage.tsx](file:///Users/wsd/Data/Busytok/busytok/apps/gui/src/pages/ProvidersPage.tsx))
- `model.added` / `model.add.failed`: `{ provider_id, model_id }`
- `model.updated` / `model.update.failed`: `{ provider_id, model_id }`
- `model.tags.updated` / `model.tags.update.failed`: `{ provider_id, model_id, tags }`
- `model.deleted` / `model.delete.failed`: `{ provider_id, model_id }`

### Partial-success note

In the partial-success path (§3 step 6: provider created, model failed), emit `provider.added` once (the provider did get created) **and** `model.add.failed` with the model error. On retry of `model.create` only, emit `model.added` / `model.add.failed` as appropriate — do not re-emit `provider.added`.

### Tests

`ProvidersPage.test.tsx` must assert that each mutation path emits the expected event code on both success and failure (mock `reportFrontendEventSafely` and assert with `expect.objectContaining({ event_code: ... })`). The deleted `ModelsSection.test.tsx` assertions for `model.added` / `model.deleted` must be migrated into `ProvidersPage.test.tsx`.

---

## 9. File Impact Summary

### Deleted
- `apps/gui/src/components/ProfilesSection.tsx`
- `apps/gui/src/components/ProfilesSection.test.tsx`
- `apps/gui/src/components/ModelsSection.tsx` (functionality absorbed into ProvidersPage)
- `apps/gui/src/components/ModelsSection.test.tsx`

### Modified (GUI)
- `apps/gui/src/pages/ProvidersPage.tsx` — full rewrite (provider cards + inline models + create form)
- `apps/gui/src/pages/ProvidersPage.test.tsx` — update tests for new structure
- `apps/gui/src/api/useBusytokData.ts` — remove `useProfileMutations`; also clean up stale comments referencing `ProfilesSection`/`ModelsSection` in the `useModels` hook doc comment
- `apps/gui/src/api/busytokClient.ts` — remove `profileCreate/Update/Delete` and `ProfileDto` import
- `apps/gui/src/styles/pages.css` — add `.provider-card`, `.model-row`, `.chip` classes

### Modified (CLI)
- `apps/cli/src/main.rs` — add `ProviderCommand` + `ProviderModelCommand` enums, `Command::Provider` variant, dispatch
- `apps/cli/src/commands/mod.rs` — add `pub mod provider;`

### New (CLI)
- `apps/cli/src/commands/provider.rs` — all provider+model CLI handlers

---

## 10. Testing

### GUI tests
- `ProvidersPage.test.tsx` — test provider create (with/without model), provider edit (inline header), provider delete, model inline edit, model add from card, model delete
- Remove `ProfilesSection.test.tsx` and `ModelsSection.test.tsx`
- Remove `useProfileMutations` mock from `ProvidersPage.test.tsx` (stale mock will cause lint/test failure)

### CLI tests
- Parser tests: `busytok provider add --url ... --key ... --kind ...`, `busytok provider model add <id> --name ...`
- Handler tests: mock `ControlClient`, verify correct RPC calls and output formatting
- Name derivation test: `derive_provider_name("https://api.deepseek.com/v1", "openai_compatible")` → `"deepseek_openai"`
- Collision test: existing names include `"deepseek_openai"` → derived name is `"deepseek_openai_2"`; also test when `_2` exists → should produce `_3`
- URL validation test: `validate_base_url("api.deepseek.com")` → error (missing protocol); `validate_base_url("https://api.deepseek.com/v1")` → Ok
- Edge case: `derive_provider_name("https://localhost:8080/v1", "openai_compatible")` → `"localhost_openai"` (single-part host falls back to full host)

### What NOT to test
- Backend RPCs (already tested in protocol/runtime crates)
- CSS styling (visual, not unit-testable)

---

## 11. Constraints

- Reuse all existing RPCs (`provider.list/create/update/delete/test_connection`, `model.list/create/update/delete/tags.update`). No backend changes.
- No new npm dependencies. No new Rust crates — URL host extraction in CLI uses manual string parsing (`extract_host`), not the `url` crate.
- Rust: `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test --workspace` must pass.
- Frontend: `pnpm test`, `pnpm lint`, `pnpm build` must pass.
- The existing `busytok models` command stays unchanged (backward compat).
- Provider `name` is auto-generated; the form does not collect it. Users can edit it later via provider update.
- API Key is required for provider creation (both UI and CLI).
