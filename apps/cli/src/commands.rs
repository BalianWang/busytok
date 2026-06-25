//! CLI command handlers for Busytok.
//!
//! Each handler either:
//! - Connects to the Busytok service via `ControlClient` and calls the
//!   appropriate RPC method (the normal path), OR
//! - For `scan --offline`, directly uses `BusytokSupervisor` with a
//!   temporary in-memory database to scan local files without a service.

use std::io::BufRead;
use std::path::PathBuf;

use anyhow::{Context, Result};
use busytok_config::BusytokPaths;
use busytok_control::client::ControlClient;
use busytok_domain::{AgentKind, LogSourceType};
use busytok_protocol::dto::*;
use busytok_runtime::BusytokSupervisor;
use busytok_store::Database;

/// Connect to the control server and return a client.
pub(crate) async fn connect_client() -> Result<ControlClient> {
    let endpoint = if let Ok(s) = std::env::var("BUSYTOK_SOCKET") {
        s
    } else {
        let paths = BusytokPaths::new();
        paths.control_endpoint()?
    };
    match ControlClient::connect(endpoint.clone()).await {
        Ok(client) => {
            tracing::info!(
                event_code = "cli.control.connect.ok",
                endpoint = %endpoint,
            );
            Ok(client)
        }
        Err(e) => {
            tracing::error!(
                event_code = "cli.control.connect.failed",
                endpoint = %endpoint,
                error = %e,
            );
            Err(e).context(
                "connecting to Busytok service. Open Busytok.app to start the background service, then retry."
            )
        }
    }
}

/// Call an RPC method on the control server and print the result.
async fn rpc_call(method: &str, params: serde_json::Value) -> Result<()> {
    let mut client = connect_client().await?;
    let request = ControlRequest::new(method, params);
    let response = client.call(request).await?;
    match response {
        ControlResponse::Ok(value) => {
            let output = serde_json::to_string_pretty(&value)?;
            println!("{output}");
            Ok(())
        }
        ControlResponse::Err(err) => {
            anyhow::bail!("RPC error [{}]: {}", err.code, err.message)
        }
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Show service health.
pub async fn handle_status() -> Result<()> {
    rpc_call("service.health", serde_json::json!({})).await
}

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

/// List discovered log sources.
pub async fn handle_sources_list() -> Result<()> {
    rpc_call("sources.list", serde_json::json!({})).await
}

/// Show status of a specific source.
pub async fn handle_sources_status(id: &str) -> Result<()> {
    rpc_call("sources.status", serde_json::json!({ "id": id })).await
}

/// Trigger rescan of all sources (or a specific one).
pub async fn handle_sources_rescan(source_id: Option<&str>, dry_run: bool) -> Result<()> {
    if dry_run {
        // In dry-run mode, just print the method that would be called.
        let method = "sources.rescan";
        let params = if let Some(id) = source_id {
            serde_json::json!({ "source_id": id })
        } else {
            serde_json::json!({})
        };
        println!("{method} {}", serde_json::to_string(&params)?);
        return Ok(());
    }

    let params = if let Some(id) = source_id {
        serde_json::json!({ "source_id": id })
    } else {
        serde_json::json!({})
    };
    rpc_call("sources.rescan", params).await
}

// ---------------------------------------------------------------------------
// Scan (offline)
// ---------------------------------------------------------------------------

/// Run an offline local scan without a running service.
///
/// This is the only permitted direct local runtime path. It creates an
/// in-memory database, discovers `.jsonl` files in the given path, and
/// runs the scan pipeline directly.
pub async fn handle_scan_offline(agent: &str, path: &str) -> Result<()> {
    let root = PathBuf::from(path);
    if !root.exists() {
        anyhow::bail!("path does not exist: {path}");
    }

    let db = Database::open_in_memory().context("opening in-memory database")?;
    let paths = BusytokPaths::new();
    let supervisor = BusytokSupervisor::new(db, paths);

    // Determine the agent kind from the --agent flag.
    let agent_kind = match agent {
        "claude-code" => AgentKind::ClaudeCode,
        "codex" => AgentKind::Codex,
        other => anyhow::bail!(
            "unsupported agent type for offline scan: {other} (supported: claude-code, codex)"
        ),
    };

    // Collect .jsonl files from the given path.
    let files = collect_jsonl_files(&root);
    if files.is_empty() {
        anyhow::bail!("no .jsonl files found in {path}");
    }

    let source = busytok_discovery::DiscoveredLogSource {
        agent: agent_kind,
        source_id: format!(
            "offline_{}",
            root.display()
                .to_string()
                .replace(['/', '\\'], "_")
                .trim_matches('_')
        ),
        root_path: root.clone(),
        files,
        source_type: LogSourceType::Jsonl,
        configured_by_user: true,
    };

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .context("running offline scan")?;

    println!("scan complete");
    println!("  sources:  {}", stats.sources);
    println!("  files:    {}", stats.files_scanned);
    println!("  events:   {}", stats.events_found);

    // Print the events that were ingested.
    let db = supervisor.db_handle();
    let db = db.lock().unwrap();
    let count = db.usage_event_count().context("counting events")?;
    println!("  stored:   {count}");

    Ok(())
}

/// Recursively collect all `.jsonl` files under a directory.
fn collect_jsonl_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_jsonl_files(&path));
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

// ---------------------------------------------------------------------------
// Usage
// ---------------------------------------------------------------------------

/// Show usage dashboard summary.
pub async fn handle_usage_summary() -> Result<()> {
    rpc_call("usage.dashboard", serde_json::json!({})).await
}

/// Show usage over time.
pub async fn handle_usage_timeline(
    since: Option<&str>,
    until: Option<&str>,
    agent: Option<&str>,
) -> Result<()> {
    // Normalize agent name from CLI format to runtime format.
    let normalized_agent = agent.map(|a| match a {
        "claude-code" => "claude_code",
        other => other,
    });

    let mut params = serde_json::json!({});
    if let Some(since) = since {
        params["since"] = serde_json::json!(since);
    }
    if let Some(until) = until {
        params["until"] = serde_json::json!(until);
    }
    if let Some(agent) = normalized_agent {
        params["agent"] = serde_json::json!(agent);
    }
    rpc_call("usage.timeline", params).await
}

/// List usage events.
pub async fn handle_usage_events(cursor: Option<&str>, limit: Option<u32>) -> Result<()> {
    let mut params = serde_json::json!({});
    if let Some(cursor) = cursor {
        params["cursor"] = serde_json::json!(cursor);
    }
    if let Some(limit) = limit {
        params["limit"] = serde_json::json!(limit);
    }
    rpc_call("usage.events", params).await
}

/// Show project summaries.
pub async fn handle_usage_projects() -> Result<()> {
    rpc_call("usage.projects", serde_json::json!({})).await
}

/// Show model summaries.
pub async fn handle_usage_models() -> Result<()> {
    rpc_call("usage.models", serde_json::json!({})).await
}

/// Show session summaries.
pub async fn handle_usage_sessions() -> Result<()> {
    rpc_call("usage.sessions", serde_json::json!({})).await
}

/// Export usage data in a given kind and format.
pub async fn handle_usage_export(
    kind: &str,
    format: &str,
    agent: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    // Normalize agent name from CLI format to runtime format.
    let normalized_agent = agent.map(|a| match a {
        "claude-code" => "claude_code",
        other => other,
    });

    let mut params = serde_json::json!({
        "kind": kind,
        "format": format,
    });
    if let Some(agent) = normalized_agent {
        params["agent"] = serde_json::json!(agent);
    }

    if dry_run {
        let method = "usage.export";
        println!("{method} {}", serde_json::to_string(&params)?);
        return Ok(());
    }

    rpc_call("usage.export", params).await
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// Show scan status.
pub async fn handle_diagnostics_scan_status() -> Result<()> {
    rpc_call("diagnostics.scan_status", serde_json::json!({})).await
}

/// Show store health check.
pub async fn handle_diagnostics_store_health() -> Result<()> {
    rpc_call("diagnostics.store_health", serde_json::json!({})).await
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Get current settings.
pub async fn handle_settings_get() -> Result<()> {
    rpc_call("settings.snapshot", serde_json::json!({})).await
}

/// Update settings.
///
/// `discovery_defaults` is a list of `(agent, bool)` pairs (e.g. `("claude-code", true)`).
/// `add_root` is an optional `(agent, path)` pair.
pub async fn handle_settings_update(
    timezone: Option<&str>,
    discovery_defaults: Vec<(&str, bool)>,
    add_root: Option<(&str, &str)>,
) -> Result<()> {
    let mut params = serde_json::json!({});

    if let Some(tz) = timezone {
        params["timezone"] = serde_json::json!(tz);
    }

    if !discovery_defaults.is_empty() || add_root.is_some() {
        // Build a DiscoverySettingsDto from the provided defaults.
        let mut claude_code_default_paths = None;
        let mut codex_default_paths = None;
        let mut manual_roots: Vec<serde_json::Value> = Vec::new();

        for (agent, enabled) in &discovery_defaults {
            match *agent {
                "claude-code" => claude_code_default_paths = Some(*enabled),
                "codex" => codex_default_paths = Some(*enabled),
                other => anyhow::bail!(
                    "unknown agent for --discovery-default: {other} (use claude-code or codex)"
                ),
            }
        }

        if let Some((agent, path)) = add_root {
            let client_id = match agent {
                "claude-code" => "claude_code",
                "codex" => "codex",
                other => anyhow::bail!(
                    "--add-root currently only supports claude-code or codex (got: {other})"
                ),
            };
            manual_roots.push(serde_json::json!({
                "id": "",
                "client_id": client_id,
                "root_path": path,
                "source_type": "manual_root",
            }));
        }

        // Open a single connection, get current settings, merge, and update.
        let mut client = connect_client().await?;
        let get_req = ControlRequest::new("settings.snapshot", serde_json::json!({}));
        let get_resp = client.call(get_req).await?;
        if let ControlResponse::Ok(current_val) = get_resp {
            if let Some(d) = current_val.get("discovery") {
                if claude_code_default_paths.is_none() {
                    claude_code_default_paths =
                        d.get("claude_code_default_paths").and_then(|v| v.as_bool());
                }
                if codex_default_paths.is_none() {
                    codex_default_paths = d.get("codex_default_paths").and_then(|v| v.as_bool());
                }
                // Preserve existing manual roots from current settings, avoiding duplicates.
                if let Some(roots) = d.get("manual_roots").and_then(|v| v.as_array()) {
                    for r in roots {
                        let is_dup = manual_roots.iter().any(|mr| {
                            mr.get("root_path").and_then(|v| v.as_str())
                                == r.get("root_path").and_then(|v| v.as_str())
                                && mr.get("client_id").and_then(|v| v.as_str())
                                    == r.get("client_id").and_then(|v| v.as_str())
                        });
                        if !is_dup {
                            manual_roots.push(r.clone());
                        }
                    }
                }
            }
        }

        params["discovery"] = serde_json::json!({
            "claude_code_default_paths": claude_code_default_paths.unwrap_or(true),
            "codex_default_paths": codex_default_paths.unwrap_or(true),
            "manual_roots": manual_roots,
        });

        // Reuse the same connection for the update call.
        let request = ControlRequest::new("settings.update", params);
        let response = client.call(request).await?;
        match response {
            ControlResponse::Ok(value) => {
                let output = serde_json::to_string_pretty(&value)?;
                println!("{output}");
                return Ok(());
            }
            ControlResponse::Err(err) => {
                anyhow::bail!("RPC error [{}]: {}", err.code, err.message)
            }
        }
    }

    let mut client = connect_client().await?;
    let request = ControlRequest::new("settings.update", params);
    let response = client.call(request).await?;
    match response {
        ControlResponse::Ok(value) => {
            let output = serde_json::to_string_pretty(&value)?;
            println!("{output}");
            Ok(())
        }
        ControlResponse::Err(err) => {
            anyhow::bail!("RPC error [{}]: {}", err.code, err.message)
        }
    }
}

// ---------------------------------------------------------------------------
// Prompt palette
// ---------------------------------------------------------------------------

/// Max length of prompt content in characters (matches store layer).
const CONTENT_MAX_CHARS: usize = 65_536;
/// Max length of prompt alias in characters (matches store layer).
const ALIAS_MAX_CHARS: usize = 80;

#[derive(Debug, serde::Deserialize)]
struct BatchPromptInput {
    content: Option<String>,
    alias: Option<String>,
    tags: Option<Vec<String>>,
}

#[derive(Debug, serde::Serialize, Default)]
struct BatchPromptOutput {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

// Must match store-level is_forbidden_alias_char in prompt_entries.rs
fn is_forbidden_alias_char(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '"' | '\'' | '`' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}' | '\u{feff}'
        )
}

/// Validate prompt content. Returns Some((reason_code, detail_message)) if invalid.
fn validate_content(content: &str) -> Option<(&'static str, String)> {
    if content.trim().is_empty() {
        return Some((
            "empty_content",
            "prompt content must not be empty".to_string(),
        ));
    }
    if content.chars().count() > CONTENT_MAX_CHARS {
        return Some((
            "content_too_large",
            format!("prompt content must be at most {CONTENT_MAX_CHARS} characters"),
        ));
    }
    None
}

/// Validate alias. Returns Some((reason_code, detail_message)) if invalid.
fn validate_alias(alias: &str) -> Option<(&'static str, String)> {
    let alias = alias.trim();
    if alias.is_empty() {
        return None;
    }
    if alias.chars().any(is_forbidden_alias_char) {
        return Some((
            "invalid_alias",
            "alias must not contain whitespace, quotes, or backticks".to_string(),
        ));
    }
    if alias.chars().count() > ALIAS_MAX_CHARS {
        return Some((
            "invalid_alias",
            format!("alias must be at most {ALIAS_MAX_CHARS} characters"),
        ));
    }
    None
}

/// Attempt a single `prompts.create` RPC call, reconnecting on transport error.
/// Returns `(entry_id, entry_alias)` on success.
async fn create_prompt_entry(
    client: &mut ControlClient,
    req: &PromptCreateRequestDto,
) -> Result<(String, Option<String>)> {
    let params = serde_json::to_value(req)?;
    let request = ControlRequest::new("prompts.create", params);
    match client.call(request).await {
        Ok(ControlResponse::Ok(value)) => {
            // The response is a ReadEnvelopeDto<PromptEntryDto>.
            let envelope: ReadEnvelopeDto =
                serde_json::from_value(value).context("deserializing prompts.create response")?;
            let id = envelope
                .data
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let alias = envelope
                .data
                .get("alias")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            Ok((id, alias))
        }
        Ok(ControlResponse::Err(err)) => {
            let code = err.code;
            let message = err.message;
            anyhow::bail!("RPC error [{}]: {}", code, message)
        }
        Err(e) => {
            anyhow::bail!("transport error: {}", e)
        }
    }
}

/// Emit a skipped-line JSONL record to stdout.
fn emit_skipped(index: usize, reason: &str, detail: String, alias: Option<&str>) -> Result<()> {
    let output = BatchPromptOutput {
        status: "skipped".to_string(),
        index: Some(index),
        reason: Some(reason.to_string()),
        detail: Some(detail),
        alias: alias.map(|s| s.to_string()),
        ..Default::default()
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

/// Handle `busytok prompt create`.
pub async fn handle_prompt_create(
    content: Option<String>,
    alias: Option<String>,
    tags: Vec<String>,
    batch: bool,
) -> Result<()> {
    if batch {
        handle_prompt_create_batch().await
    } else {
        let content = content.ok_or_else(|| {
            anyhow::anyhow!("--content is required (or use --batch to read from stdin)")
        })?;

        // Client-side pre-validation (same rules as batch mode and frontend dialog).
        if let Some((_reason, detail)) = validate_content(&content) {
            anyhow::bail!(detail);
        }

        let alias_clean = alias.filter(|a| !a.trim().is_empty());
        if let Some(ref alias_val) = alias_clean {
            if let Some((_reason, detail)) = validate_alias(alias_val) {
                anyhow::bail!(detail);
            }
        }

        let params = serde_json::json!({
            "content": content,
            "alias": alias_clean,
            "tags": tags,
        });
        rpc_call("prompts.create", params).await
    }
}

/// Batch-create prompt entries from stdin JSONL.
async fn handle_prompt_create_batch() -> Result<()> {
    let stdin = std::io::stdin();
    let reader = std::io::BufReader::new(stdin);
    let mut any_skipped = false;
    let mut client: Option<ControlClient> = None;

    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Parse JSON line.
        let input: BatchPromptInput = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                emit_skipped(index, "invalid_json", e.to_string(), None)?;
                any_skipped = true;
                continue;
            }
        };

        // Validate content.
        let content = input.content.as_deref().unwrap_or("");
        if let Some((reason, detail)) = validate_content(content) {
            emit_skipped(index, reason, detail, input.alias.as_deref())?;
            any_skipped = true;
            continue;
        }

        // Validate alias.
        if let Some(ref alias) = input.alias {
            if let Some((reason, detail)) = validate_alias(alias) {
                emit_skipped(index, reason, detail, input.alias.as_deref())?;
                any_skipped = true;
                continue;
            }
        }

        let alias_for_rpc = input.alias.filter(|a| !a.trim().is_empty());

        let req = PromptCreateRequestDto {
            content: content.to_string(),
            alias: alias_for_rpc,
            tags: input.tags.unwrap_or_default(),
        };

        // Lazy-connect to daemon before first RPC call.
        if client.is_none() {
            match connect_client().await {
                Ok(c) => client = Some(c),
                Err(e) => {
                    emit_skipped(index, "rpc_error", e.to_string(), req.alias.as_deref())?;
                    any_skipped = true;
                    continue;
                }
            }
        }

        // Attempt RPC create, with one reconnect retry on transport error.
        match create_prompt_entry(client.as_mut().unwrap(), &req).await {
            Ok((id, entry_alias)) => {
                let output = BatchPromptOutput {
                    status: "created".to_string(),
                    id: Some(id),
                    alias: entry_alias,
                    ..Default::default()
                };
                println!("{}", serde_json::to_string(&output)?);
            }
            Err(e) => {
                let err_str = e.to_string();
                let reason = if err_str.contains("an alias with this name already exists") {
                    "alias_conflict"
                } else {
                    "rpc_error"
                };
                emit_skipped(index, reason, err_str, req.alias.as_deref())?;
                any_skipped = true;

                // If transport error, attempt reconnect for the next entry.
                if e.to_string().contains("transport error") {
                    client = connect_client().await.ok();
                }
            }
        }
    }

    if any_skipped {
        anyhow::bail!("some entries were skipped (see JSONL output for details)");
    }
    Ok(())
}
