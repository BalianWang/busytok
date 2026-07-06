# CLI delegate: add `--bind-provider` / `--bind-model` bound-field flags

**Status:** draft  
**Date:** 2026-07-05  
**Requires:** PR #74 (subagent provider/model binding, merged to main as `0640fac`)

## Motivation

PR #74 moved provider/model binding from profile config to the subagent
itself (`bound_provider_id` + `bound_model_id` columns on
`subagent_logical_subagents`, NOT NULL). The CLI `delegate` command currently
hardcodes these fields to `None`:

```rust
// apps/cli/src/commands_subagent.rs:48-49
bound_provider_id: None,
bound_model_id: None,
```

This means:

1. **New subagent creation always fails** ("bound_provider_id and
   bound_model_id are both required to create a subagent")
2. **Reusing existing subagents works** (the stored bound fields are used)
   for BOTH reuse paths:
   - `--id <UUID>` shortcut
   - `--subagent <NAME> --cwd <DIR>` when an active logical subagent with
     that name already exists in the repo scope

Important correction from the current `main` code: name-based reuse does
NOT fail. `resolve_by_name()` first looks up active rows by `(repo_hash,
name)` and returns the existing row when it finds exactly one; it only
enters the create path (where bound fields are required) when there are
zero active matches. Therefore the CLI must NOT eagerly reject the
`(None, None)` case, because that would break valid name-based reuse.

The `models list` command already works and returns provider→model mappings.
The `delegate` command needs flags to let the user specify which provider and
model to bind a new subagent to.

## Global Constraints

- **No breaking changes to existing CLI flags.** The current `--model` flag
  on `delegate` maps to `model_override` (task-level override) and is
  **fully functional for the reuse path** (existing subagents). It must
  remain working. Renaming it would break existing users/scripts.
- **Reuse existing CLI infrastructure.** The `model.list` RPC typed path
  already exists in `apps/cli/src/commands/models.rs` — reuse the
  `ModelListRequestDto` / `ModelListResponseDto` / `ControlRequest::new`
  pattern, do NOT hand-roll a raw `client.call("models.list", ...)` + ad-hoc
  JSON parse path.
- **Display bound fields as IDs, not names.** `SubagentDetailDto` carries
  `bound_provider_id: String` and `bound_model_id: String` (IDs, not
  human-readable names). Resolving IDs to names would require an extra
  `provider.list` / `model.list` RPC per display, which is out of scope for
  this task. Display the IDs directly; users can cross-reference with
  `busytok models` / `busytok subagent show` output.
- **Rust coverage must remain >90%** (`cargo llvm-cov --workspace --fail-under-lines 90`).
- `cargo clippy --workspace --all-targets -- -D warnings` must pass.
- `cargo test --workspace` must pass.
- `cargo fmt --all` must pass.

## Design

### CLI flags

Add two optional flags to `busytok delegate`:

```
--bind-provider <PROVIDER>   Provider ID to bind a new subagent to
--bind-model <MODEL>         Model ID to bind a new subagent to
```

**Naming choice: `--bind-provider` / `--bind-model` (not `--provider` / `--model`).**

The delegate command already has `--model` which maps to `model_override`
(a task-level model override for an existing binding). Renaming the existing
`--model` (Option A in the original draft) would be a breaking change: the
`model_override` feature is **fully functional for the reuse path** — users
delegate to existing subagents with `--model <override>` to temporarily
override the bound model for one task. This is not "non-functional" as the
original draft claimed.

Options considered:
- Option A: rename existing `--model` (task override) to `--model-override`.
  **Rejected** — breaking change, invalidates existing usage of `--model`
  on the reuse path.
- Option B: name the new bound field `--bind-model` (and `--bind-provider`).
  **Chosen** — zero breaking changes, "bind" clearly distinguishes creation-
  time binding from task-time override.
- Option C: keep `--model` as the bound model, add `--model-override` for
  task override. **Rejected** — `--model`'s semantics would silently change
  from task override to bound field, which is also a breaking change.

The `bind-` prefix makes the semantics unambiguous: `--bind-*` for creation-
time binding, `--model` for task-time override.

**Both-or-neither rule:** if one `--bind-*` is provided without the other,
the CLI errors out before making the RPC call (client-side validation,
matching the service-side "both or neither" spec §3.3).

**Do NOT reject `(None, None)` in the CLI.** That case must still be
forwarded to the service so existing subagents can be reused by either
`--id` or `--subagent + --cwd`. The service already rejects the create path
when no existing subagent matches and both bound fields are absent.

### Auto-resolution (stretch goal)

If `--bind-model` is given but `--bind-provider` is not, call the existing
typed `model.list` RPC helper path and auto-resolve the provider:

- Exactly 1 match → use it, print info message to stderr
- 0 matches → error: "model not found in catalog"
- >1 matches (same model ID across multiple providers) → error: "model
  <X> is available from multiple providers; use --bind-provider to disambiguate:
  <list>"

This makes the common case (unique model names) ergonomic.

Implementation constraint: reuse the existing CLI model-catalog plumbing in
`apps/cli/src/commands/models.rs`:
- RPC method name is `model.list` (singular), not `models.list`
- request DTO is `ModelListRequestDto`
- response DTO is `ModelListResponseDto`
- `ControlResponse` is matched as `Ok(value)` / `Err(err)` — the existing
  module-private `unwrap_ok(resp: ControlResponse) -> Result<serde_json::Value>`
  helper in `commands_subagent.rs` (line 184) already encapsulates this
  pattern; reuse it.

Do NOT hand-roll a raw `client.call("models.list", ...)` + ad-hoc JSON parse
path in `commands_subagent.rs`; that would duplicate the existing typed
request/response contract and drift from the current CLI infrastructure.

### Passing to RPC

The resolved/bind value maps directly to the existing DTO fields:

```rust
SubagentDelegateRequestDto {
    // ...
    bound_provider_id: Some(provider),  // was None
    bound_model_id: Some(model),        // was None
}
```

No service-side or protocol changes needed — the DTO already has these
fields (spec §3.3).

### Subagent list display

`busytok subagent list` should show the bound provider and model IDs for each
subagent. The `SubagentDetailDto` already includes `bound_provider_id` and
`bound_model_id` (added in PR #74). Per Global Constraints, display IDs
directly (no ID→name resolution).

Current `print_array` output format (no header, 3 columns):
```
f4409090-...                          test01                cold
```

Target output (add BINDING column between NAME and STATUS, no header —
matching the existing headerless format):
```
f4409090-...                          test01                deepseek01/deepseek-chat  cold
```

The `busytok subagent show <name>` detail view should also include these
fields as IDs.

## Implementation tasks

### Task 1 — Add `--bind-provider` / `--bind-model` flags to the Delegate command

**File:** `apps/cli/src/main.rs` (~line 83-102)

Add two new fields to the `Delegate` variant. Do NOT touch the existing
`model` field (it stays as `model_override` for task-level override):

```rust
Delegate {
    #[arg(long)]
    subagent: String,
    #[arg(long)]
    id: Option<String>,
    #[arg(long, default_value = ".")]
    cwd: String,
    #[arg(long)]
    profile: String,
    #[arg(long)]
    intent: Option<String>,
    #[arg(long)]
    model: Option<String>,          // UNCHANGED — task-level override
    #[arg(long)]
    timeout: Option<u64>,
    #[arg(long, default_value = "text", value_parser = ["json", "text"])]
    output: String,
    /// Provider ID to bind a new subagent to (required with --bind-model for new subagents)
    #[arg(long)]
    bind_provider: Option<String>,  // NEW
    /// Model ID to bind a new subagent to (required with --bind-provider for new subagents)
    #[arg(long)]
    bind_model: Option<String>,     // NEW
    /// The task prompt (positional)
    prompt: String,
}
```

Update the call site at `main.rs:601` to pass the two new args to
`handle_delegate` (see Task 3 for the new signature). The existing `model`
arg is passed through unchanged.

**Also update the `command_name_returns_delegate_for_delegate_variant`
test** at `main.rs:723-736`: it constructs `Command::Delegate { ... }` as a
struct literal and will fail to compile after adding the two new fields.
Add `bind_provider: None, bind_model: None,` to the struct literal (before
the `prompt` field) to keep the existing assertion unchanged.

### Task 2 — Client-side validation + auto-resolution

**File:** `apps/cli/src/commands_subagent.rs` — new helper function

**New imports needed** (add to the existing `use` block at top of file):
```rust
use busytok_protocol::dto::{
    ModelCatalogEntryDto, ModelListRequestDto, ModelListResponseDto,
    SubagentDelegateRequestDto, SubagentDeleteRequestDto, SubagentListRequestDto,
    SubagentResolveRequestDto, SubagentTasksRequestDto,
};
```
(`ModelCatalogEntryDto`, `ModelListRequestDto`, `ModelListResponseDto` are
new; the rest are already imported.)

The helper reuses the existing module-private `unwrap_ok` function
(`commands_subagent.rs:184`) which returns `Result<serde_json::Value>` from
a `ControlResponse`. This matches the pattern already used by
`handle_delegate` (line 58) and `handle_list` (line 80).

```rust
/// Resolve (provider_id, model_id) from --bind-provider / --bind-model flags.
/// Returns:
/// - Ok(Some((provider, model))) when the bound fields are fully resolved
/// - Ok(None) for the valid reuse-path passthrough case `(None, None)`
/// - Err(...) for asymmetric or ambiguous input
async fn resolve_bound_fields(
    client: &mut ControlClient,
    bind_provider: Option<String>,
    bind_model: Option<String>,
) -> Result<Option<(String, String)>> {
    match (bind_provider, bind_model) {
        (Some(p), Some(m)) => {
            // Both given — pass through directly.
            Ok(Some((p, m)))
        }
        (None, None) => {
            // Important: do NOT reject this case in the CLI. It is valid for
            // BOTH reuse paths:
            //   - --id <UUID>
            //   - --subagent <NAME> --cwd <DIR> when an active subagent with
            //     that name already exists in the repo scope
            // The service resolver will only reject it if the request falls
            // through to the create path (0 active matches).
            Ok(None)
        }
        (Some(_), None) => {
            anyhow::bail!("--bind-model is required when --bind-provider is given")
        }
        (None, Some(model)) => {
            // Auto-resolve provider from the model catalog.
            // Reuse the typed model.list RPC path (same as `commands/models.rs`).
            let req = ModelListRequestDto {
                provider_id: None,
                tags: vec![],
                include_disabled: false, // never bind to disabled provider/model
            };
            let resp = client
                .call(ControlRequest::new(
                    "model.list",
                    serde_json::to_value(&req)?,
                ))
                .await?;
            let value = unwrap_ok(resp)?;
            let entries: Vec<ModelCatalogEntryDto> =
                serde_json::from_value::<ModelListResponseDto>(value)?.models;
            // include_disabled=false already filters disabled entries; the
            // extra model_enabled && provider_enabled filter is defense-in-depth.
            let matches: Vec<_> = entries
                .iter()
                .filter(|e| e.model_id == model && e.model_enabled && e.provider_enabled)
                .collect();
            match matches.len() {
                0 => anyhow::bail!("model '{}' not found in catalog", model),
                1 => {
                    let pid = &matches[0].provider_id;
                    eprintln!("  (auto-resolved provider: {})", pid);
                    Ok(Some((pid.clone(), model)))
                }
                _ => {
                    let providers: Vec<_> = matches.iter().map(|e| &e.provider_id).collect();
                    anyhow::bail!(
                        "model '{}' is available from multiple providers: {:?}\n\
                         Use --bind-provider to disambiguate.",
                        model, providers
                    )
                }
            }
        }
    }
}
```

### Task 3 — Wire into `handle_delegate`

**File:** `apps/cli/src/commands_subagent.rs`

Update the function signature to accept the two new flags, call
`resolve_bound_fields`, and populate the DTO. Important: `(None, None)` is a
PASSTHROUGH case, not a CLI error. The CLI only errors on asymmetric input
(`Some/None`) or on `None/Some` when auto-resolution cannot resolve it.

The existing `model` parameter (task-level override) is UNCHANGED — it still
maps to `model_override` on the DTO.

```rust
pub async fn handle_delegate(
    subagent: String,
    id: Option<String>,
    cwd: String,
    profile: String,
    intent: Option<String>,
    model: Option<String>,            // UNCHANGED — task-level override
    timeout: Option<u64>,
    output: String,
    prompt: String,
    bind_provider: Option<String>,    // NEW
    bind_model: Option<String>,       // NEW
) -> Result<()> {
    let mut client = connect().await?;

    // Resolve bound fields before building the DTO.
    let (bound_provider_id, bound_model_id) = match resolve_bound_fields(
        &mut client,
        bind_provider,
        bind_model,
    )
    .await? {
        Some((p, m)) => (Some(p), Some(m)),
        None => (None, None), // valid reuse-path passthrough
    };

    let req = SubagentDelegateRequestDto {
        subagent_name: subagent,
        subagent_id: id,
        cwd,
        profile,
        intent,
        prompt,
        prompt_artifact_ref: None,
        timeout_seconds: timeout,
        model_override: model,   // UNCHANGED
        source_harness: Some("cli".to_string()),
        source_session_id: None,
        bound_provider_id,
        bound_model_id,
    };
    let resp = client
        .call(ControlRequest::new(
            "subagent.delegate",
            serde_json::to_value(&req)?,
        ))
        .await
        .context("subagent.delegate RPC failed")?;
    let data = unwrap_ok(resp)?;
    print_delegate(&data, &output)
}
```

Update the call site in `apps/cli/src/main.rs` (around line 601) to pass
`bind_provider` and `bind_model` as the last two args.

### Task 4 — Update `subagent list` display

**File:** `apps/cli/src/commands_subagent.rs`

**Do NOT modify the shared `print_array` helper.** `print_array` (line 217)
is a generic array-envelope printer used by both `handle_list` (key
`"subagents"`) and `handle_tasks` (key `"tasks"`). Stuffing binding-column
logic into it would expand the scope from "enhance subagent list" to "also
change the `subagent tasks` text layout", which is out of scope and makes
the shared helper harder to maintain.

Instead, add a dedicated renderer for the subagent list and route only
`handle_list` through it. `handle_tasks` continues to call the unchanged
`print_array`.

**Step 1 — add `print_subagent_list`** (new function, next to
`print_array`):

```rust
/// Print the `subagents` array with a BINDING column
/// (`bound_provider_id`/`bound_model_id`). Per Global Constraints, display
/// IDs directly (no ID→name resolution). Used only by `handle_list`;
/// `handle_tasks` keeps the generic `print_array` path.
fn print_subagent_list(value: &serde_json::Value, output: &str) -> Result<()> {
    print_json_or(value, output, |v| {
        let arr = v.get("subagents").and_then(|a| a.as_array());
        match arr {
            Some(items) if items.is_empty() => println!("(no subagents)"),
            Some(items) => {
                for item in items {
                    let id = item.get("id").and_then(|s| s.as_str()).unwrap_or("?");
                    let name = item
                        .get("name")
                        .and_then(|s| s.as_str())
                        .or_else(|| item.get("subagent_name").and_then(|s| s.as_str()))
                        .unwrap_or("?");
                    let bound_provider = item
                        .get("bound_provider_id")
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    let bound_model = item
                        .get("bound_model_id")
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    // Post-migration-0007 the columns are NOT NULL, so this
                    // fallback is purely defensive against malformed JSON.
                    let binding = if bound_provider.is_empty() && bound_model.is_empty() {
                        "-".to_string()
                    } else {
                        format!("{bound_provider}/{bound_model}")
                    };
                    let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("?");
                    println!("{id:<36} {name:<20} {binding:<40} {status}");
                }
            }
            None => println!("{}", serde_json::to_string_pretty(v).unwrap_or_default()),
        }
    })
}
```

**Step 2 — route `handle_list` through the new renderer.** In `handle_list`
(line 81) change:

```rust
print_array(&data, "subagents", "text")
```

to:

```rust
print_subagent_list(&data, "text")
```

`handle_tasks` (line 123) is untouched and keeps calling
`print_array(&data, "tasks", "text")`.

The `{binding:<40}` width accommodates typical `provider-id/model-id` pairs.
The `-` fallback is purely defensive (post-migration-0007 the columns are
NOT NULL, but the JSON might be malformed).

### Task 5 — Update `subagent show` display

**File:** `apps/cli/src/commands_subagent.rs` — `print_detail` function (line 240)

The current `print_detail` prints `id/name/status`. Add `provider` and
`model` lines showing the bound field IDs (per Global Constraints, no
ID→name resolution):

```rust
fn print_detail(value: &serde_json::Value, output: &str) -> Result<()> {
    print_json_or(value, output, |v| {
        let id = v.get("id").and_then(|s| s.as_str()).unwrap_or("?");
        let name = v
            .get("name")
            .and_then(|s| s.as_str())
            .or_else(|| v.get("subagent_name").and_then(|s| s.as_str()))
            .unwrap_or("?");
        let bound_provider = v
            .get("bound_provider_id")
            .and_then(|s| s.as_str())
            .unwrap_or("?");
        let bound_model = v
            .get("bound_model_id")
            .and_then(|s| s.as_str())
            .unwrap_or("?");
        let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("?");
        println!("id:       {id}");
        println!("name:     {name}");
        println!("provider: {bound_provider}");
        println!("model:    {bound_model}");
        println!("status:   {status}");
    })
}
```

### Task 6 — Update tests

**File:** `apps/cli/src/commands_subagent.rs` — existing tests module

Coverage target: maintain >90% Rust line coverage (`cargo llvm-cov --workspace --fail-under-lines 90`).

Tests to update / add:

- **Update** `command_name_returns_delegate_for_delegate_variant` in
  `apps/cli/src/main.rs:723-736`: add `bind_provider: None, bind_model: None,`
  to the `Command::Delegate { ... }` struct literal so it compiles after
  Task 1 adds the two new fields. The existing `assert_eq!(command_name(&cmd),
  "delegate")` assertion is unchanged.
- **Update** `handle_delegate_invokes_subagent_delegate_rpc_*` (lines 814, 839):
  pass the two new `bind_provider` / `bind_model` args (as `None, None` for
  the existing reuse-path tests). The existing `--model` (model_override)
  tests continue to pass `model` unchanged.
- **Update** the `print_detail` unit tests (lines 318-411): update expected
  output strings to include the `provider:` / `model:` lines. Add
  `bound_provider_id` / `bound_model_id` to the test JSON fixtures. The
  existing `print_array` tests (lines 346-381) stay unchanged — Task 4 does
  NOT touch `print_array`, so `print_array_text_empty_tasks_uses_key_in_message`
  (line 377) and the other `print_array` tests continue to pass as-is.
- **Add** `print_subagent_list` unit tests (new, next to the existing
  `print_array` tests):
  - empty `subagents` array → prints `(no subagents)`
  - non-empty array with `bound_provider_id` / `bound_model_id` → each row
    includes `{provider}/{model}` in the BINDING column
  - non-empty array with missing `bound_provider_id` / `bound_model_id`
    (malformed JSON) → BINDING column falls back to `-`
  - `json` output mode → pretty-prints the JSON payload (delegates to
    `print_json_or`)
- **Add** test: `handle_delegate` with `--bind-provider` + `--bind-model` →
  DTO includes both as `Some(...)`.
- **Add** test: `handle_delegate` with `--bind-model` only → auto-resolves
  provider via `model.list` (use a `RuntimeControl` wrapper that returns a
  canned `ModelListResponseDto`, following the `ModelsRuntime` pattern in
  `commands/models.rs:288`).
- **Add** test: `handle_delegate` with `--bind-model` + ambiguous (multiple
  providers) → error message includes provider list.
- **Add** test: `handle_delegate` with `--bind-provider` only (no
  `--bind-model`) → client-side error "--bind-model is required when
  --bind-provider is given".
- **Add** test: `handle_delegate` with neither bind flag on REUSE path
  (`--id <UUID>`) → still reaches `subagent.delegate` and does not fail
  client-side (DTO has `bound_provider_id: None, bound_model_id: None`).
- **Add** test: `resolve_bound_fields(None, None)` returns `Ok(None)`.
- **Add** test: `resolve_bound_fields(Some(p), Some(m))` returns
  `Ok(Some((p, m)))` without calling the RPC (no auto-resolution).
- **Add** test: `print_detail` with missing `bound_provider_id` / `bound_model_id`
  in JSON → falls back to `?` (defensive).

## NOT in scope

- GUI changes (the GUI already has provider/model selection in the subagent
  creation flow)
- Profile-level provider binding (removed in PR #74; binding is per-subagent)
- `busytok provider add/create` CLI command (provider CRUD remains
  GUI-only for now; can be a follow-up)
- ID→name resolution in `subagent list` / `subagent show` display (would
  require an extra RPC per display; deferred)
- Renaming the existing `--model` flag (kept as `model_override` for
  task-level override; no breaking changes)
