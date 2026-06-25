# Prompt Palette Three Action Modes

Date: 2026-06-24
Status: Spec — pending implementation

## Goal

Replace the current two-mode prompt action (Copy / Paste — both write the user's
clipboard) with three clean modes:

| Mode | Writes clipboard | Pastes into active app |
|---|---|---|
| Copy only | ✅ | — |
| Paste only | — (saves + restores) | ✅ |
| Copy & paste | ✅ | ✅ |

Configurable in **Settings → Prompt Palette → Default action** (a
`SegmentedControl`).  Also move the **Prompt Palette Paste** diagnostics row
from the Diagnostics section into the Prompt Palette section so
permission/paste status lives next to the default-action control.

## Background — current execution flow

`executePromptAction()` in [`apps/gui/src/lib/promptPaletteActions.ts:180`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/lib/promptPaletteActions.ts:180)
**unconditionally writes the clipboard first** at line 187
(`await deps.writeClipboard(entry.content)`), then branches:

- `action === "copy"` → done (clipboard already written).
- `action === "paste"` → `beforePaste()` → `pasteActiveApp()` (Rust
  `CMD+V` injection via CoreGraphics in
  [`macos.rs:80`](/Users/wsd/Data/Busytok/busytok/apps/gui/src-tauri/src/prompt_palette_native/macos.rs:80)).

So both modes touch the clipboard. The new "Paste only" mode must paste
**without permanently modifying** the clipboard by saving it before the write
and restoring it after the paste completes.

The `PromptActionDto` enum lives at
[`crates/busytok-protocol/src/dto.rs:822`](/Users/wsd/Data/Busytok/busytok/crates/busytok-protocol/src/dto.rs:822)
(currently `Copy | Paste`).  It derives `TS` so the TypeScript type is
auto-generated.

The Settings default-action control is a `SegmentedControl` at
[`apps/gui/src/pages/SettingsPage.tsx`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/pages/SettingsPage.tsx)
under the **Prompt Palette** section.

## Design

### §1 Protocol — new `PasteOnly` variant

In `crates/busytok-protocol/src/dto.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum PromptActionDto {
    Copy,
    Paste,
    PasteOnly,       // ← new
}
```

`#[serde(rename_all = "lowercase")]` maps this to `"paste_only"` in JSON and
the auto-generated TypeScript type.

### §2 Capabilities — clipboard read permission

In `apps/gui/src-tauri/capabilities/default.json`, add
`"clipboard-manager:allow-read-text"` alongside the existing
`"clipboard-manager:allow-write-text"`:

```json
"clipboard-manager:allow-read-text",
"clipboard-manager:allow-write-text",
```

(This is needed to read the old clipboard before the paste-only write.)

### §3 Action execution — `executePromptAction`

In `apps/gui/src/lib/promptPaletteActions.ts`, refactor `executePromptAction`
so the unconditional clipboard-write at line 187 is skipped for `paste_only`,
and a new branch handles the save→write→paste→restore flow.

Pseudo-code for the new branch:

```ts
if (action === "paste_only") {
  let oldClipboard = "";
  try { oldClipboard = await readText(); } catch { /* empty clipboard — continue */ }

  await deps.writeClipboard(entry.content);

  // Reuse the existing paste flow (beforePaste → pasteActiveApp → recordUse)
  // — identical to the "paste" branch after the clipboard write.
  // … same steps as action === "paste" from line 230 onward …

  // Restore the old clipboard — best-effort, failure is non-blocking because
  // the paste already succeeded.
  try { await deps.writeClipboard(oldClipboard); } catch { /* telemetry */ }
  return result;
}
```

**Error handling:** if the restore fails, the paste already succeeded — log
a `WARN` telemetry event (`gui.prompt_palette.paste_only_restore_failed`) and
return the successful paste result.  Do NOT throw or roll back.

**Refactoring note:** the paste branch (lines 217–256) and the new paste_only
branch share the same `beforePaste → pasteActiveApp → recordUse` path.
Extract a small helper `async function runPaste(deps, entry, surface, action)`
so the two branches are a few lines each and don't duplicate logic.

### §4 Settings UI — Default action + move diagnostics

**Default action `SegmentedControl`** (existing, under "Prompt Palette" section):

Current options: `["paste", "copy"]` → `{ value: "paste", label: "Paste" }, { value: "copy", label: "Copy" }`.

Add a third:

```tsx
{ value: "paste_only", label: "Paste only" }
```

The control sends the selected value to the server-backed
`prompt_palette_default_action` setting (existing flow — unchanged).

**Move paste-status diagnostics.**  The Diagnostics section currently has a
"Prompt Palette Paste" row (with `pasteStatusText()` + an "Open System
Settings" button). Move that entire `<SettingsRow>` into the **Prompt
Palette** section, directly below (or above) the Default action control, so
permission status sits next to the action-mode selector.

Remove the old row from the Diagnostics section.

### §5 Controller wiring — no changes

`PromptPaletteOverlayController.tsx` already reads `defaultAction` and passes
it through to `executePromptAction`.  The enum extension automatically flows
— the `Enter` key executes `defaultAction` (whatever it is), and the `⌘K`
actions menu offers Copy/Paste as before (the menu options do not need to
list Paste Only — it's the default-action Enter path).

### §6 Testing

- **Protocol regression:** a Rust test confirming `PasteOnly` serialises to
  `"paste_only"` and deserialises back.
- **`executePromptAction` unit test** (`promptPaletteActions.test.ts`):
  - `paste_only` writes clipboard, calls beforePaste/pasteActiveApp, records
    paste_attempted outcome.
  - `paste_only` restores old clipboard after successful paste (mock
    `readText` → "old content", assert final `writeClipboard("old content")`).
  - `paste_only` does NOT fail when clipboard restore throws (assert outcome
    is still `paste_attempted`).
- **Existing `copy` and `paste` tests must still pass** (no regression).
- **Settings test** (`SettingsPage.test.tsx`): the Defaut action control
  offers three options; the paste-status row is inside the Prompt Palette
  section (not Diagnostics).

### §7 Out of scope

- Rust-side changes — `post_command_v` / `macos.rs` / `windows.rs` are
  untouched.
- The `⌘K` actions menu — it still offers Copy / Paste / Edit / …; Paste
  Only is only a *default-action* setting, not a per-prompt action.
- Per-prompt action override (e.g. "this specific prompt always pastes
  only") — not in this spec.

## File modifications

| File | Change |
|---|---|
| `crates/busytok-protocol/src/dto.rs` | `PromptActionDto` — add `PasteOnly` variant |
| `apps/gui/src-tauri/capabilities/default.json` | Add `clipboard-manager:allow-read-text` |
| `apps/gui/src/lib/promptPaletteActions.ts` | `executePromptAction` — skip forced write for `paste_only`, add save→write→paste→restore branch + extract shared `runPaste` helper |
| `apps/gui/src/lib/promptPaletteActions.test.ts` | 3 new tests (paste_only success, restore, restore-failure non-blocking) |
| `apps/gui/src/pages/SettingsPage.tsx` | Add "Paste only" to SegmentedControl options; move paste-status row from Diagnostics → Prompt Palette section |
| `apps/gui/src/pages/SettingsPage.test.tsx` | Assert 3 options + paste status in Prompt Palette section |

## Linked

- Prior research: current paste/copy flow traced in
  `promptPaletteActions.ts` / `promptPalettePasteBridge.ts` / `macos.rs`.
- Related: `docs/bugs/2026-06-24-prompt-palette-window-service-status-race.md`
  (panel service-status race).
